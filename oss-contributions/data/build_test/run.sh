#!/bin/bash
# Build + test top-3 shortlisted repos; log everything.
set -x
cd /home/user/oss-try
T() { local s=$(date +%s); "$@"; local rc=$?; echo "ELAPSED $(( $(date +%s) - s ))s: $1 $2 $3" >&2; return $rc; }

# ---------- pandas ----------
if [ ! -d pandas ]; then T git clone --depth 50 https://github.com/pandas-dev/pandas; fi
cd pandas
python3 -m venv .venv && . .venv/bin/activate
T pip install -q -r requirements-dev.txt > ../pandas_deps.log 2>&1; echo "deps rc=$?"
T pip install -q -ve . --no-build-isolation -Ceditable-verbose=true > ../pandas_build.log 2>&1; echo "build rc=$?"
python -c "import pandas; print(pandas.__version__)"
T pytest pandas -n 4 -q -m "not slow and not network and not db and not single_cpu" -p no:cacheprovider > ../pandas_test.log 2>&1; echo "test rc=$?"
deactivate; cd ..

# ---------- scikit-learn ----------
if [ ! -d scikit-learn ]; then T git clone --depth 50 https://github.com/scikit-learn/scikit-learn; fi
cd scikit-learn
python3 -m venv .venv && . .venv/bin/activate
T pip install -q numpy scipy cython meson-python ninja pytest pytest-xdist joblib threadpoolctl pandas matplotlib > ../sklearn_deps.log 2>&1; echo "deps rc=$?"
T pip install -v --editable . --no-build-isolation --config-settings editable-verbose=true > ../sklearn_build.log 2>&1; echo "build rc=$?"
python -c "import sklearn; sklearn.show_versions()"
T pytest sklearn -n 4 -q -p no:cacheprovider > ../sklearn_test.log 2>&1; echo "test rc=$?"
deactivate; cd ..

# ---------- matplotlib ----------
if [ ! -d matplotlib ]; then T git clone --depth 50 https://github.com/matplotlib/matplotlib; fi
cd matplotlib
git fetch --depth 50 --tags origin > /dev/null 2>&1   # setuptools-scm needs a tag for the version
python3 -m venv .venv && . .venv/bin/activate
T pip install -q -r requirements/dev/build-requirements.txt -r requirements/testing/all.txt > ../mpl_deps.log 2>&1; echo "deps rc=$?"
T pip install -v --no-build-isolation --editable . > ../mpl_build.log 2>&1; echo "build rc=$?"
python -c "import matplotlib; print(matplotlib.__version__)"
T pytest lib/matplotlib -n 4 -q -p no:cacheprovider > ../mpl_test.log 2>&1; echo "test rc=$?"
deactivate; cd ..
echo ALLDONE
