# Sample project

A small Python project for trying Bridle. `calc.py` has one bug: `add()` subtracts instead of adding, so the unit test
fails until the bug is fixed.

Run the test from this folder:

```bash
python -m unittest -q
```

To have Bridle fix the bug, follow [fix a failing test](../../docs/examples/fix-a-failing-test.md). That guide copies this
folder, so the agent edits a copy and not the repository.
