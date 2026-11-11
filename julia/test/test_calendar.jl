# Calendar cadences (spec/esm-sources.md §2): the shared `calendar-cadence`
# scenario — periods, anchors, and a `temporal` Provider over the corpus.
using Dates

_cal_flatten(x::AbstractVector) = reduce(vcat, (_cal_flatten(e) for e in x); init = Float64[])
_cal_flatten(x::Real) = Float64[x]
_cal_corder(a::AbstractArray) = vec(permutedims(a, reverse(1:ndims(a))))

@testset "calendar cadences — the shared scenario" begin
    corpus = joinpath(@__DIR__, "..", "..", "conformance", "corpus")
    sc = JSON.parsefile(joinpath(corpus, "scenarios", "calendar-cadence.json"))

    @testset "periods" begin
        for c in sc["periods"]
            p = CalendarPeriod(c["iso"])
            @test (p.months, p.seconds) == (c["months"], c["seconds"])
        end
        for bad in sc["bad_periods"]
            err = try
                CalendarPeriod(bad)
                nothing
            catch e
                e
            end
            @test err isa CalendarPeriodError && occursin("esm_period_invalid", sprint(showerror, err))
        end
        @test_throws CalendarPeriodError CalendarPeriod("P99999999999999999999D")
        @test CalendarPeriod(Month(1)) == CalendarPeriod("P1M")
        @test CalendarPeriod(Hour(3)) == CalendarPeriod("PT3H")
    end

    u(t) = datetime2unix(t)
    for c in sc["cadences"]
        @testset "cadence $(c["id"])" begin
            temporal = SourceTemporal(
                Float64(c["start_s"]), c["frequency"], c["file_period"];
                stop = haskey(c, "end_s") ? Float64(c["end_s"]) : nothing
            )
            p = Provider(
                Cache(; root = mktempdir(), offline = true), "file:///unused.nc";
                format = "netcdf", temporal = temporal,
                window = (Float64(c["window_s"][1]), Float64(c["window_s"][2]))
            )
            @test refresh_times(p) == Float64.(c["refresh_times_s"])
            for at in c["at"]
                t = Float64(at["t_s"])
                a = record_anchor(temporal, t)
                fa = file_anchor(temporal, a)
                @test u(a) == at["record_anchor_s"]
                @test u(fa) == at["file_anchor_s"]
                @test record_in_file(temporal, a, fa) == at["record"]
                @test u(next_record_anchor(temporal, t)) == at["next_record_anchor_s"]
            end
        end
    end

    cache = Cache(; root = joinpath(corpus, "cache"), offline = true)
    for (name, src) in sc["providers"]
        @testset "provider $name" begin
            var = src["variables"][1]
            for rps in (1, 2)
                temporal = SourceTemporal(
                    Float64(src["start_s"]), src["frequency"], src["file_period"];
                    stop = Float64(src["end_s"]), time_dim = src["time_dim"],
                    records_per_sample = rps
                )
                url = EsmTemplate(
                    src["url_template"];
                    substitutions = Dict{String, String}(src["substitutions"])
                )
                p = Provider(
                    cache, url; format = src["format"], variables = String.(src["variables"]),
                    temporal = temporal,
                    window = (Float64(src["window_s"][1]), Float64(src["window_s"][2]))
                )
                @test refresh_times(p) == Float64.(src["refresh_times_s"])
                # The same bracket asked of the Provider rather than the temporal.
                q = Provider(
                    cache, url; format = src["format"], variables = String.(src["variables"]),
                    temporal = SourceTemporal(
                        Float64(src["start_s"]), src["frequency"], src["file_period"];
                        stop = Float64(src["end_s"]), time_dim = src["time_dim"]
                    ),
                    window = (Float64(src["window_s"][1]), Float64(src["window_s"][2])),
                    records_per_sample = rps
                )
                for smp in src["samples"], prov in (p, q)
                    smp["records_per_sample"] == rps || continue
                    nds = sample_at(prov, Float64(smp["t_s"]))
                    f = nds[var]
                    # the fixture nests in file (row-major) order
                    @test _cal_corder(f.data) == _cal_flatten(smp[var])
                    if rps == 2
                        @test nds.coords[src["time_dim"]].data == Float64.(smp["time_s"])
                    end
                end
                # A group's epochs are the window start (where the record in
                # effect there takes over) and the refresh times, nothing
                # outside the window.
                g = PrefetchGroup(Dict("x" => p))
                @test boundaries(g) ==
                    sort!(unique!(vcat(Float64(src["window_s"][1]), refresh_times(p))))
                # `prefetch` warms the files over the window, as Python and Rust do.
                @test length(prefetch(p)) == length(
                    unique(file_anchor(temporal, t) for t in refresh_times(p))
                )
            end
        end
    end

    # Which records serve a run window, and which windows are refused
    # (spec/esm-sources.md §2.6).
    for c in sc["windows"]
        @testset "window $(c["id"])" begin
            temporal = SourceTemporal(
                Float64(c["start_s"]), c["frequency"], c["file_period"];
                stop = haskey(c, "end_s") ? Float64(c["end_s"]) : nothing,
                records_per_sample = c["records_per_sample"]
            )
            make() = Provider(
                Cache(; root = mktempdir(), offline = true), "file:///unused.nc";
                format = "netcdf", temporal = temporal,
                window = (Float64(c["window_s"][1]), Float64(c["window_s"][2]))
            )
            if haskey(c, "error")
                err = try
                    make()
                    nothing
                catch e
                    e
                end
                @test err isa WindowNotCovered
                @test startswith(sprint(showerror, err), c["error"])
            else
                p = make()
                @test window_records(p) == Float64.(c["window_records_s"])
                @test refresh_times(p) == Float64.(c["refresh_times_s"])
                # A group schedules it even with no refresh times: its first
                # epoch starts at the window start (spec/prefetch.md §3.2).
                @test boundaries(PrefetchGroup(Dict("x" => p))) == sort!(
                    unique!(vcat(Float64(c["window_s"][1]), Float64.(c["refresh_times_s"])))
                )
            end
        end
    end

    # A two-record bracket over the window's last interval reads the record
    # after the window, so prefetch warms that record's file too (§2.6).
    for (name, src) in sc["providers"]
        haskey(src, "prefetch") || continue
        @testset "prefetch $name reaches the bracket successor" begin
            url = EsmTemplate(
                src["url_template"];
                substitutions = Dict{String, String}(src["substitutions"])
            )
            for c in src["prefetch"]
                temporal = SourceTemporal(
                    Float64(src["start_s"]), src["frequency"], src["file_period"];
                    stop = c["open_ended"] ? nothing : Float64(src["end_s"]),
                    time_dim = src["time_dim"], records_per_sample = c["records_per_sample"]
                )
                p = Provider(
                    cache, url; format = src["format"], variables = String.(src["variables"]),
                    temporal = temporal,
                    window = (Float64(c["window_s"][1]), Float64(c["window_s"][2]))
                )
                if c["missing_url"] === nothing
                    @test !isempty(prefetch(p))
                else
                    err = try
                        prefetch(p)
                        nothing
                    catch e
                        e
                    end
                    @test err isa CacheMiss && err.url == c["missing_url"]
                end
            end
        end
    end
end
