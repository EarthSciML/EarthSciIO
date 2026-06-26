"""Make the in-repo ``earthsciio`` package importable for the test suite.

Until the Python package gains a ``pyproject.toml`` (owned by the language-core
bead ``esio-9nb.2``, which installs ``earthsciio`` properly), this ensures
``import earthsciio`` resolves from the repo root under any pytest invocation —
so the registry-dispatch tests run standalone on this branch. Harmless once the
package is pip-installed: the source tree simply shadows the install for tests.
"""

import os
import sys

_ROOT = os.path.dirname(os.path.abspath(__file__))
if _ROOT not in sys.path:
    sys.path.insert(0, _ROOT)
