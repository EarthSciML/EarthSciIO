# esm URL templates (spec/esm-sources.md §1): the shared `esm-url-templates`
# scenario, and esm mirror templates through a Provider and the cache.
using Dates
import NCDatasets

@testset "esm URL templates — the shared scenario" begin
    sc = JSON.parsefile(
        joinpath(@__DIR__, "..", "..", "conformance", "corpus", "scenarios", "esm-url-templates.json")
    )
    for case in sc["cases"]
        @testset "$(case["id"])" begin
            anchor = case["anchor_s"] === nothing ? nothing : Float64(case["anchor_s"])
            subs = Dict{String, String}(String(k) => String(v) for (k, v) in case["substitutions"])
            if haskey(case, "expect")
                @test expand_esm_template(case["template"], anchor; substitutions = subs) == case["expect"]
            else
                err = try
                    expand_esm_template(case["template"], anchor; substitutions = subs)
                    nothing
                catch e
                    e
                end
                @test err isa EsmTemplateError
                if err isa EsmTemplateError
                    @test err.code == case["error"]
                    msg = sprint(showerror, err)
                    @test occursin(case["error"], msg)
                    for m in get(case, "mentions", Any[])
                        @test occursin(m, msg)
                    end
                end
            end
            if haskey(case, "names")
                @test esm_template_names(case["template"]) == String.(case["names"])
            end
        end
    end
    # a DateTime anchor is the same instant as its Unix seconds
    @test expand_esm_template("{date:%Y%m%d%H}", DateTime(2016, 2, 29, 7)) == "2016022907"
end

@testset "esm templates — mirrors expanded at the anchor" begin
    mktempdir() do dir
        mirror = joinpath(dir, "mirror")
        mkpath(mirror)
        for (m, v) in ((1, 1.0), (2, 10.0))
            NCDatasets.NCDataset(joinpath(mirror, "emis_agr_0$m.nc"), "c") do ds
                NCDatasets.defDim(ds, "x", 2)
                vv = NCDatasets.defVar(ds, "v", Float64, ("x",))
                vv[:] = [v, v + 1]
            end
        end
        @test_throws EsmTemplateError EsmTemplate("file:///x/{sector}.nc")
        url = EsmTemplate(
            "file://$dir/missing/emis_{sector}_{date:%m}.nc";
            substitutions = Dict("sector" => "agr"),
            mirrors = ["file://$mirror/emis_{sector}_{date:%m}.nc"]
        )
        @test_throws ArgumentError Provider(
            Cache(; root = joinpath(dir, "c0")), url; format = "netcdf",
            mirrors = ["file:///elsewhere.nc"]
        )
        jan = datetime2unix(DateTime(2016, 1, 1))
        feb = datetime2unix(DateTime(2016, 2, 1))
        p = discrete_provider(
            Cache(; root = joinpath(dir, "cache")), url, [jan, feb];
            format = "netcdf", variables = ["v"]
        )
        @test materialize(p, jan)["v"].data == [1.0, 2.0]
        @test materialize(p, feb + 3600)["v"].data == [10.0, 11.0]
        entries = prefetch(p)
        @test length(entries) == 2
        # cached under the PRIMARY url's key, and the manifest names it
        @test entries[2].key == cache_key("file://$dir/missing/emis_agr_02.nc")
        @test entries[2].manifest.url == "file://$dir/missing/emis_agr_02.nc"
        # every candidate failing names them all
        bad = discrete_provider(
            Cache(; root = joinpath(dir, "cache2")),
            EsmTemplate("file://$dir/nope/{date:%m}.nc"; mirrors = ["file://$dir/nope2/{date:%m}.nc"]),
            [jan]; format = "netcdf"
        )
        msg = try
            materialize(bad, jan)
            ""
        catch e
            sprint(showerror, e)
        end
        @test occursin("nope2/01.nc", msg)
    end
end
