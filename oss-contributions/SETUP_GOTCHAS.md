# Setup gotchas: building and testing the top 3 repos

I cloned, built from source and ran the full test suite of the top 3 shortlisted projects on **2026-10-16**, so you hit fewer surprises.

**Machine.** Ubuntu 24.04 container, 4 cores, 15 GB RAM, Python 3.13, gcc 13. It sits behind a proxy that only allows git, PyPI and apt traffic, which is stricter than your laptop but showed which steps download things at build time.

## Results

| Repo (commit) | Build time | Test command | Result |
|---|---|---|---|
| scikit-learn (`main`, 1.9.dev) | 2 m 43 s | `pytest sklearn -n 4` | ✅ **37,614 passed**, 13,289 skipped, 0 failed (9 m 11 s) |
| pandas (`main`, 3.2.0.dev0+302) | 4 m 9 s | `pytest pandas -n 4 -m "not slow and not network and not db and not single_cpu"` | ✅ **245,802 passed**. 19 failed on the first run, all from an OS tzdata gotcha (#4 below); after the fix they pass (940/940 in the affected files) |
| matplotlib (`main`, 3.11.0.dev2372) | 1 m 42 s | `pytest lib/matplotlib -n 4` | ✅ **8,614 passed**, 1 failed (`test_axeswidget_interactive`: no `tkinter`, gotcha #6) |

`-n 4` needs `pytest-xdist` (`pip install pytest-xdist`). It gives about a 3–4× speed-up on a 4-core laptop.

## Gotchas (in the order you'd hit them)

1. **Install a C/C++ toolchain first.** All three build C/C++/Cython extensions with meson. On Ubuntu/WSL: `sudo apt install build-essential python3-dev`. On Windows, use WSL2 rather than native MSVC for your first weeks.

2. **pandas: don't use a shallow clone without tags.** With `git clone --depth 50` the build "succeeds" but pandas reports its version as `0+untagged…`. pyarrow then refuses to import it, so *every* test errors at `conftest.py`. Fix it with a normal clone, or `git fetch --tags --shallow-since=2025-06-01 origin main`, and then **delete `build/`** so meson regenerates the version file. Matplotlib (setuptools-scm) needs tags for the same reason.

3. **Meson downloads C dependencies at build time.** pandas fetches `fast_float` and `xsimd` from GitHub archive URLs (`subprojects/*.wrap`). Matplotlib fetches FreeType (savannah.nongnu.org), HarfBuzz, libraqm, SheenBidi and qhull. On a university network or proxy that blocks these hosts, the build fails with `is the internet available?`. Workarounds:
   - pandas: `git clone -b v8.2.10 https://github.com/fastfloat/fast_float subprojects/fast_float-8.2.10` (and the same for `xsimd-14.2.0`), then copy `subprojects/packagefiles/<name>/meson.build` into each.
   - matplotlib: use system libraries where they are new enough, e.g. `pip install -e . --no-build-isolation -Csetup-args=-Dsystem-qhull=true` after `sudo apt install libqhull-dev`. Ubuntu 24.04's `libraqm` (0.10.1) is **too old** (needs ≥0.10.4), so keep the bundled one or clone it into `subprojects/` and apply the patch in `subprojects/packagefiles/`.

4. **pandas timezone tests fail on Ubuntu 24.04+ (19 failures).** Ubuntu moved legacy zone names like `US/Pacific` and `US/Eastern` into a separate package, so pyarrow says `US/Pacific not found in timezone database`. Fix: `sudo apt install tzdata-legacy`. This is environmental, not a pandas bug, so don't report it upstream.

5. **matplotlib switched from `requirements/*.txt` to PEP 735 dependency groups.** `requirements/dev/...` no longer exists. Use `pip install --group build --group test` (or `--group dev`), which **needs pip ≥ 25.1**. Debian/Ubuntu's system pip is 24.0, so run `pip install -U pip` inside your venv first.

6. **`ModuleNotFoundError: tkinter` in one matplotlib test.** Install `sudo apt install python3-tk` (needed for the interactive-backend tests). This is harmless to ignore otherwise.

7. **Use `--no-build-isolation` for editable installs, and install the build deps yourself.** This holds for all three. Editable meson builds **rebuild on import**: the first `import pandas` after a `git pull` can take a minute, which is normal.

8. **Don't run the whole suite while iterating.** Run the file you touch (`pytest pandas/tests/groupby/test_x.py -q`). The full runs above are for a final sanity check, and CI runs everything anyway.

9. **scikit-learn was the smoothest:** `pip install numpy scipy cython meson-python ninja pytest pytest-xdist` → `pip install -e . --no-build-isolation` → green. That makes it a good first build if you're nervous about toolchains.

The exact scripts used are in `data/build_test/` (`run.sh` and the follow-up runs).
