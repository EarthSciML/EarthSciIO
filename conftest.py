"""Test-suite wiring: in-repo imports, and the optional-extra skip marker.

``pythonpath = ["."]`` in ``pyproject.toml`` already makes ``import earthsciio``
resolve from the repo root; the ``sys.path`` insert below keeps that working
under a bare ``pytest`` invocation from anywhere.

The rest of this file is the ``needs_format`` marker. A test that decodes a
format whose stack is an optional extra must SKIP, not fail, when that extra is
absent — otherwise a lean ``pip install -e ".[test]"`` reports 16 red tests that
say nothing about the code under test. Most modules express that with a
module-level ``pytest.importorskip`` (see ``tests/test_zarr_reader.py``), but
``test_provider.py`` and ``test_readers.py`` are mostly format-independent and
would over-skip, so those tests are marked individually.

The marker deliberately asks the REGISTRY rather than naming modules itself:

    @pytest.mark.needs_format("netcdf")

resolves through ``format_registry.missing_requirements("netcdf")``, the same
declaration the reader carries and the conformance dumper consults. One source
of truth for "what does this format need" — adding a backend to a reader's
``REQUIRES`` updates the reader, the dumper and these skips together.
"""

import os
import sys

import pytest

_ROOT = os.path.dirname(os.path.abspath(__file__))
if _ROOT not in sys.path:
    sys.path.insert(0, _ROOT)


def pytest_configure(config):
    config.addinivalue_line(
        "markers",
        "needs_format(name): skip unless the registry reports that format's "
        "decode stack is installed in this environment",
    )


def pytest_runtest_setup(item):
    """Skip a ``needs_format``-marked test when the format's extra is absent."""
    markers = list(item.iter_markers(name="needs_format"))
    if not markers:
        return
    # Imported lazily: the sys.path insert above has to land first, and an
    # unmarked run should not pay for importing the package here at all.
    from earthsciio.registry import format_registry

    for marker in markers:
        fmt = marker.args[0]
        missing = format_registry.missing_requirements(fmt)
        if missing:
            pytest.skip(
                f"the {fmt!r} reader needs {', '.join(missing)}, not installed "
                f"in this environment (install the matching extra)"
            )
