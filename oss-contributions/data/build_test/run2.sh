#!/bin/bash
cd /home/user/oss-try
T() { local s=$(date +%s); "$@"; local rc=$?; echo "ELAPSED $(( $(date +%s) - s ))s: $1 $2 $3" >&2; return $rc; }
cd pandas && . .venv/bin/activate
T pip install -ve . --no-build-isolation -Ceditable-verbose=true > ../pandas_build.log 2>&1; echo "pandas build rc=$?"
python -c "import pandas; print(pandas.__version__)"
T pytest pandas -n 4 -q -m "not slow and not network and not db and not single_cpu" -p no:cacheprovider > ../pandas_test.log 2>&1; echo "pandas test rc=$?"
deactivate; cd ../matplotlib
pip --version
. .venv/bin/activate; pip install -q -U pip
T pip install -q --group build --group test > ../mpl_deps.log 2>&1; echo "mpl deps rc=$?"
T pip install -v --no-build-isolation --editable . > ../mpl_build.log 2>&1; echo "mpl build rc=$?"
python -c "import matplotlib; print(matplotlib.__version__)"
T pytest lib/matplotlib -n 4 -q -p no:cacheprovider > ../mpl_test.log 2>&1; echo "mpl test rc=$?"
echo ALLDONE
