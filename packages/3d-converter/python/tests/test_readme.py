"""README.md's examples are doctests: each runs against the installed wheel
and prints what the README says it prints."""

import doctest
from pathlib import Path

README = Path(__file__).resolve().parents[1] / "README.md"


def test_readme_examples_run_as_written() -> None:
    result = doctest.testfile(str(README), module_relative=False, optionflags=doctest.ELLIPSIS)
    assert result.attempted >= 10
    assert result.failed == 0
