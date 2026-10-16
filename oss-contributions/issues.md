# Recommended issues: checklist

Every issue below was opened on GitHub on **2026-10-16** and was **open, unassigned, with no linked or referenced open PR**.
Items marked ⚠️ have an informal claim or an old offer in the thread: read it and ask before starting.
Attack plans are in `REPORT.md`, and verification notes (including excluded issues) are in `data/raw/verified.md`.

Before starting any of them: re-check that the issue is still free, then claim it the way the project expects (pandas `/take`, netbox volunteer → assignment, others: comment your plan).

## Tier 1

### pandas-dev/pandas (claim with `/take`)
- [ ] [#53706](https://github.com/pandas-dev/pandas/issues/53706) Easy: regression test for `SeriesGroupBy.nlargest(as_index=False)` (Needs Tests)
- [ ] [#57837](https://github.com/pandas-dev/pandas/issues/57837) Easy–Med: test for `shift(freq=DateOffset)` across DST (Needs Tests)
- [ ] [#45800](https://github.com/pandas-dev/pandas/issues/45800) Easy: test for `df.agg([custom_func, "sum"])` (Needs Tests)
- [ ] [#42851](https://github.com/pandas-dev/pandas/issues/42851) Easy–Med: test that `value_counts(dropna=False)` keeps `pd.NA` (Needs Tests)

### scikit-learn/scikit-learn
- [ ] [#35033](https://github.com/scikit-learn/scikit-learn/issues/35033) Easy: fix RFECV `cv_results_` shape docs ⚠️ (a user asked for confirmation on 09-25)
- [ ] [#33958](https://github.com/scikit-learn/scikit-learn/issues/33958) Easy: glossary entry for "dataframe" (sub-item of the narwhals meta-issue)
- [ ] [#33457](https://github.com/scikit-learn/scikit-learn/issues/33457) Medium: add `sample_weight` to ONE unclaimed estimator (meta-issue)
- [ ] [#31595](https://github.com/scikit-learn/scikit-learn/issues/31595) Medium: property vs attribute docstring rendering + consistency test

### matplotlib/matplotlib
- [ ] [#30165](https://github.com/matplotlib/matplotlib/issues/30165) Easy: clarify `Patch.get_transform` docstring
- [ ] [#28745](https://github.com/matplotlib/matplotlib/issues/28745) Easy–Med: document `linestyle` as an `:mpltype:` ⚠️ (an Mar-2026 question went unanswered)
- [ ] [#31809](https://github.com/matplotlib/matplotlib/issues/31809) Medium: ipympl figure de-registered from pyplot (debug)

### secdev/scapy
- [ ] [#3625](https://github.com/secdev/scapy/issues/3625) Easy–Med: PIM Register / Register-Stop layers (RFC 7761)
- [ ] [#4680](https://github.com/secdev/scapy/issues/4680) Medium: configurable pcap snaplen / buffer size
- [ ] [#4055](https://github.com/secdev/scapy/issues/4055) Easy–Med: unify route/iface resync (`network_resync()`)
- [ ] [#3670](https://github.com/secdev/scapy/issues/3670) Medium–Hard: `MultipleTypeField` + `FieldListField` build bug

### pydata/xarray
- [ ] [#11721](https://github.com/pydata/xarray/issues/11721) Easy: claim ONE item from the deprecations tracker
- [ ] [#10591](https://github.com/pydata/xarray/issues/10591) Medium: structured-dtype `_FillValue` with Zarr v2 ⚠️ (maintainer asked the reporter first)
- [ ] [#8239](https://github.com/pydata/xarray/issues/8239) Easy: one repo-review item per PR (e.g. PC170, MY104)

### numpy/numpy
- [ ] [#28028](https://github.com/numpy/numpy/issues/28028) Easy: "Debugging from an IDE" docs subsection
- [ ] [#28311](https://github.com/numpy/numpy/issues/28311) Easy: docstrings for `ndarray.__abs__` / `ndarray.__new__` only
- [ ] [#32272](https://github.com/numpy/numpy/issues/32272) Easy–Med: document the `.npz` container ⚠️ (two offers in Aug, no PR)

### pylint-dev/pylint
- [ ] [#9556](https://github.com/pylint-dev/pylint/issues/9556) Easy–Med: `using-constant-test` false positive on `SystemExit.code`
- [ ] [#10630](https://github.com/pylint-dev/pylint/issues/10630) Medium: W0102 for `os.getenv` / `os.environ` defaults (+ #4659)
- [ ] [#9977](https://github.com/pylint-dev/pylint/issues/9977) Medium: honour isort custom sections in import-order checks

### netbox-community/netbox (volunteer, then wait for assignment before any PR)
- [ ] [#23412](https://github.com/netbox-community/netbox/issues/23412) Easy: `API_TOKEN_PEPPERS` raw-string docs
- [ ] [#23248](https://github.com/netbox-community/netbox/issues/23248) Easy–Med: tags column in search results
- [ ] [#23237](https://github.com/netbox-community/netbox/issues/23237) Medium: clickable related objects for custom fields

## Tier 2

### sktime/sktime
- [ ] [#8384](https://github.com/sktime/sktime/issues/8384) Easy–Med: weekly CI job against nightly dependency wheels ⚠️ (two unconfirmed offers)
- [ ] [#11329](https://github.com/sktime/sktime/issues/11329) Medium: speed up global recursive reduction (pick an unclaimed bottleneck) ⚠️
- [ ] [#7285](https://github.com/sktime/sktime/issues/7285) Easy–Med: move detection plotting helpers to `detection.plotting` ⚠️ (stalled PR #7300)

### pytorch/pytorch (EasyCLA required)
- [ ] [#74602](https://github.com/pytorch/pytorch/issues/74602) Medium: `scatter_object_list` rank bug on subgroups
- [ ] [#89197](https://github.com/pytorch/pytorch/issues/89197) Easy: regression tests for bool tensors on gloo ⚠️ (test-PR offer 10-04)
- [ ] [#74809](https://github.com/pytorch/pytorch/issues/74809) Easy: regression test for gather of permuted input ⚠️ (test-PR offer 10-04)

### pytest-dev/pytest
- [ ] [#12235](https://github.com/pytest-dev/pytest/issues/12235) Easy: explain `OutcomeException(BaseException)` in docstring
- [ ] [#8208](https://github.com/pytest-dev/pytest/issues/8208) Easy–Med: docs example plugin with resource + parametrized fixtures
- [ ] [#13266](https://github.com/pytest-dev/pytest/issues/13266) Easy–Med: investigate LaTeX docs build failure (may already be fixed upstream)

### mitmproxy/mitmproxy
- [ ] [#8088](https://github.com/mitmproxy/mitmproxy/issues/8088) Medium: Happy Eyeballs (RFC 8305) for upstream connections
- [ ] [#8012](https://github.com/mitmproxy/mitmproxy/issues/8012) Easy–Med: delete all flows matching a filter
- [ ] [#6888](https://github.com/mitmproxy/mitmproxy/issues/6888) Medium: Windows MSI upgrade leaves stale entries (needs Windows)

### mlflow/mlflow (DCO: `git commit -s`)
- [ ] [#11484](https://github.com/mlflow/mlflow/issues/11484) Medium: Docker build args/secrets for `build-docker` (design first)
- [ ] [#14841](https://github.com/mlflow/mlflow/issues/14841) Easy–Med (TS): URL-persisted experiment search filter ⚠️ (Mar-2025 volunteer gone)

## Tier 3

### marimo-team/marimo (CLA via PR comment)
- [ ] [#2918](https://github.com/marimo-team/marimo/issues/2918) Easy–Med: config to hide download options (Python + TS)
- [ ] [#2405](https://github.com/marimo-team/marimo/issues/2405) Medium (TS): "Copy as Altair chart" for data_explorer
- [ ] [#3825](https://github.com/marimo-team/marimo/issues/3825) Medium (TS): persist collapsed headings ⚠️ (waiting on scope)

**Total: 44 issues across 14 repos** (33 with no caveat, 11 marked ⚠️).
