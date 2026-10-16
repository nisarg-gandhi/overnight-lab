# Where to start contributing to open source: ranked report

*Prepared 2026-10-16 for a 3rd-year CS undergrad (Python, ML/DL basics, computer networks).
All numbers come from GitHub on 2026-10-16 (see [Method](#method--scoring-rubric)).
Every recommended issue was opened and read on GitHub that day. Each one was open, had no assignee, and had no open PR.*

Files in this folder:

| File | What it is |
|---|---|
| `REPORT.md` | This report: ranking, shortlist deep-dives, roadmap, etiquette, programs |
| `candidates.csv` | All 72 candidate repos with every metric and sub-score |
| `issues.md` | Flat checklist of every recommended issue |
| `data/` | Raw inputs and the scripts that produce `candidates.csv` (`git_activity.py`, `score.py`, `raw/verified.md` per-issue notes) |

---

## TL;DR

1. **Start with pandas.** Its `Needs Tests` issues are bugs already fixed on `main` that only need a regression test. A maintainer re-labelled them on 2026-10-14. You claim one with `/take`, so nobody can race you.
2. **Then try scikit-learn or xarray docs/tracking issues.** Both have fast reviewers (95% / 77% of recent issues answered). Their tracking issues are split into small claimable items.
3. **Use your networking background on scapy** (packet crafting, pure Python, 71% PR merge rate, 92% of issues answered) **or NetBox** (assign-before-PR workflow, 92% merge rate). They have far less competition than the ML repos.
4. **Avoid racing on fresh "good first issue" labels in 2026.** In mlflow, narwhals, PEFT, sktime and jax, new beginner issues collect a PR or a pile of "can I take this?" comments within hours to days, often from AI-agent-driven accounts. Several projects now have explicit AI-contribution policies. Read them, and write your own code (details under [Etiquette](#prissue-etiquette-2026-edition)).

### Summary table (the shortlist)

Tier 1 projects each have at least 3 verified free issues and a high fit. Tier 2 projects are good, but their free issues are contested or few. Tier 3 projects are good orgs whose free work is mostly frontend/TypeScript. The score is out of 100 ([rubric](#method--scoring-rubric)).

| # | Repo | Tier | Score (/100) | PR merge rate | Recent issues answered | Why it fits you | Verified issues | Best first issue |
|---|---|---|---|---|---|---|---|---|
| 1 | [pandas-dev/pandas](https://github.com/pandas-dev/pandas) | 1 | 90.0 | 67% | 91% | Core data tool, `/take` claiming, test-only first PRs | 4 | [#53706](https://github.com/pandas-dev/pandas/issues/53706) |
| 2 | [scikit-learn/scikit-learn](https://github.com/scikit-learn/scikit-learn) | 1 | 91.3 | 52% | 95% | The ML library; best-in-class docs/reviews | 4 | [#35033](https://github.com/scikit-learn/scikit-learn/issues/35033) |
| 3 | [matplotlib/matplotlib](https://github.com/matplotlib/matplotlib) | 1 | 92.9 | 71% | 93% | Highest score; `first-contribution` label; GSoC 2026 org | 3 | [#30165](https://github.com/matplotlib/matplotlib/issues/30165) |
| 4 | [secdev/scapy](https://github.com/secdev/scapy) | 1 | 84.6 | 71% | 92% | Pure-Python packet library; direct use of your networking course | 4 | [#4680](https://github.com/secdev/scapy/issues/4680) |
| 5 | [pydata/xarray](https://github.com/pydata/xarray) | 1 | 83.0 | 51% | 77% | Labelled arrays (ML/climate); fresh tracking issue with claimable items | 3 | [#11721](https://github.com/pydata/xarray/issues/11721) |
| 6 | [numpy/numpy](https://github.com/numpy/numpy) | 1 | 79.3 | 71% | 89% | Foundation of everything; docs work maintainers asked for this month | 3 | [#28028](https://github.com/numpy/numpy/issues/28028) |
| 7 | [pylint-dev/pylint](https://github.com/pylint-dev/pylint) | 1 | 77.5 | 74% | 51%¹ | Teaches Python ASTs/inference; 105 `Needs PR` false-positive bugs | 3 | [#9556](https://github.com/pylint-dev/pylint/issues/9556) |
| 8 | [netbox-community/netbox](https://github.com/netbox-community/netbox) | 1 | 71.6 | 92% | 58%¹ | Network source-of-truth (IPAM/DCIM), Django; assign-before-PR | 3 | [#23412](https://github.com/netbox-community/netbox/issues/23412) |
| 9 | [sktime/sktime](https://github.com/sktime/sktime) | 2 | 83.7 | 49% | 72% | Time-series ML; very welcoming, but GFIs are crowded | 3 | [#8384](https://github.com/sktime/sktime/issues/8384) |
| 10 | [pytorch/pytorch](https://github.com/pytorch/pytorch) | 2 | 76.2 | 51% | 75% | Deep learning core; `torch.distributed` bugs fit networking | 3 | [#74602](https://github.com/pytorch/pytorch/issues/74602) |
| 11 | [pytest-dev/pytest](https://github.com/pytest-dev/pytest) | 2 | 75.1 | 56% | 75% | Tool you use daily; tiny docs PRs available | 3 | [#12235](https://github.com/pytest-dev/pytest/issues/12235) |
| 12 | [mitmproxy/mitmproxy](https://github.com/mitmproxy/mitmproxy) | 2 | 70.5 | 54% | 69% | TLS-intercepting proxy; asyncio networking work | 3 | [#8088](https://github.com/mitmproxy/mitmproxy/issues/8088) |
| 13 | [mlflow/mlflow](https://github.com/mlflow/mlflow) | 2 | 88.8 | 64% | 86% | MLOps platform; high score, but GFIs are claimed within hours | 2 | [#11484](https://github.com/mlflow/mlflow/issues/11484) |
| 14 | [marimo-team/marimo](https://github.com/marimo-team/marimo) | 3 | 80.4 | 78% | 72% | Reactive notebooks; free issues are mostly TypeScript | 3 | [#2918](https://github.com/marimo-team/marimo/issues/2918) |

¹ These projects triage mainly with **labels**, not comments, so the comment-based "answered" metric undercounts them. See [Caveats](#caveats).

The top-scoring projects whose issues did not survive verification are covered in [Honorable mentions](#honorable-mentions-high-score-but-no-free-issues-today): zulip (88.3), transformers (86.6), darts (79.8), PEFT (79.0), pymc (77.0) and others. For each, the section says when to come back.

---

## Method & scoring rubric

### Candidate pool (72 repos)

| Category | Repos |
|---|---|
| ML/DL (27) | pytorch, lightning, torchmetrics, torchvision, ignite, transformers, datasets, diffusers, peft, accelerate, trl, huggingface_hub, scikit-learn, jax, flax, equinox, optuna, mlflow, keras, sktime, aeon, darts, PyG, kornia, spaCy, pymc, deepchem |
| AI/LLM tooling (10) | langchain, llama_index, litellm, pydantic-ai, autogen, vllm, dspy, smolagents, haystack, instructor |
| Data (9) | pandas, polars, duckdb-python, ibis, narwhals, xarray, dask, marimo, pandera |
| Dev tools (8) | textual, rich, typer, pytest, pylint, mypy, hatch, ruff |
| Networking (10) | scapy, httpx, aiohttp, urllib3, requests, mitmproxy, dnspython, netmiko, netbox, fastapi |
| Beginner-friendly orgs (8) | zulip, sympy, networkx, napari, scikit-image, matplotlib, mesa, numpy |

### Data sources (all collected 2026-10-16)

- **Commit activity** comes from a commits-only git clone of each repo since 2024-10-16 (`data/git_activity.py`). It records commits and distinct authors in the last 90 days, plus *new authors in the last 180 days* (people whose first commit in the two-year window is recent). That last count stands in for how many outsiders actually get merged. Bot authors are excluded.
- **Merge rate** comes from GitHub search: PRs opened 2026-04-16 → 2026-08-16 versus how many of those were merged. The window is 2–6 months old, so PRs have had time to resolve. PyTorch is special-cased because it lands PRs through ghstack and marks them with a `Merged` label.
- **Responsiveness**: of the issues opened 2026-08-01 → 2026-09-30, the share that has at least one comment today. This was measured for 55 of the 72 repos. Unmeasured repos get a neutral half score.
- **Beginner-issue supply** is the number of open, unassigned issues with any beginner label: `good first issue`, `help wanted`, `easy`, `sprintable`, `Easy to Fix`, `Good Second Issue`, `contributions welcome`, `beginner friendly`, and repo-specific ones such as `contrib-good-first-issue`, `Needs PR`, `status: needs owner`, `first-contribution` and `Needs Tests`.
- **CONTRIBUTING / setup, welcomingness and fit** are judged by hand (0–10 each), informed by the CONTRIBUTING files, mentoring-program participation and label hygiene.

### Rubric (100 points)

| Criterion | Pts | How it is computed |
|---|---|---|
| Active maintenance | 15 | 7.5 × log-scaled commits/90d (caps at 300) + 7.5 × log-scaled authors/90d (caps at 60) |
| External PR acceptance | 20 | 12 × merge rate scaled from 10% (0 pts) to 75% (full) + 8 × log-scaled new authors/180d (caps at 80) |
| Maintainer responsiveness | 20 | 20 × share of recent issues answered, scaled from 40% (0 pts) to 95% (full) |
| Beginner-issue supply | 15 | 15 × log-scaled count of unassigned beginner-labelled issues (caps at 40); ×0.6 if most of them are more than a year old |
| CONTRIBUTING / dev-setup quality | 10 | Manual: clarity, laptop-buildable without a GPU; −1 if there is no CONTRIBUTING file |
| Community welcomingness | 10 | Manual: mentoring programs, newcomer docs/meetings, tone in threads, label hygiene |
| Fit with Python/ML/networks | 10 | Manual |

The shortlist adds one more filter: **a repo needs at least 2 (ideally at least 3) issues that survive manual verification.** Verified means open, no assignee, no linked or referenced open PR, no active claim, and still relevant. That filter removed zulip, transformers, darts, PEFT and pymc from the top 15, and tiers the rest. Every exclusion and its reason is logged in `data/raw/verified.md`.

### Caveats

- **Comment-based responsiveness undercounts label-triaging projects** (pylint, polars, netbox), and bots inflate it where a bot comments on every issue. langchain was set to neutral for that reason.
- **Merge rate counts maintainers' own PRs too.** Treat it as "how healthy is the PR pipeline", not strictly "outsider acceptance". The new-authors-180d count partly corrects for this.
- **kornia** looks great on paper (82% merge rate, 159 authors/90d), but 622 issues were opened in two months, mostly automated, and 58% got no reply. Its score is high for the wrong reasons, so it is not shortlisted.
- **A few repos are effectively frozen or in maintenance mode**: httpx (no PRs), rich (1 of 74 merged), autogen (0 of 266), spaCy (1 of 21), smolagents (15 of 341), textual (17 of 118). They rank at the bottom, and that is intended.

---

## Shortlist deep-dives

A shared note for the setup sections: the C-extension projects (pandas, scikit-learn, matplotlib, numpy) build with **meson** and need a C/C++ compiler. On Linux, `sudo apt install build-essential` is enough. On Windows, use WSL2. Always work in a fresh virtual environment (`python -m venv .venv` or conda/mamba).

### 1. pandas-dev/pandas (Tier 1, score 90.0)

**What it is.** The DataFrame library.
**Why it fits.** You already use it. A strong review culture (67% merge rate, 91% of new issues answered within weeks) makes it a great CV line for ML/data roles. Its `Needs Tests` label is the most beginner-friendly work in this report. A core dev (jbrockmendel) re-labelled ~25 old bugs on 2026-10-14 to say "this behaves correctly on `main`; add a regression test". So each fix is "write one pytest test", not "debug pandas internals".

**Setup (Linux/WSL)**
```bash
git clone https://github.com/<you>/pandas && cd pandas
git remote add upstream https://github.com/pandas-dev/pandas
python -m venv .venv && source .venv/bin/activate
python -m pip install -r requirements-dev.txt
python -m pip install -ve . --no-build-isolation -Ceditable-verbose=true   # meson build, ~5–10 min
pytest pandas/tests/groupby -q -x          # run only the area you touch; the full suite is huge
pre-commit install
```
**Workflow.** No CLA. **Claim with a `/take` comment.** The bot assigns you, and you may hold at most 2 issues. Do not `/take` issues labelled `Needs Triage`/`Needs Discussion`/`Needs Info`. Add a whatsnew entry only for behaviour changes; test-only PRs don't need one. Run `pre-commit run --all-files` before pushing. **There is an automated-contributions policy:** you must disclose AI tools and fully review their output.

| Issue | Difficulty | Attack plan |
|---|---|---|
| [#53706](https://github.com/pandas-dev/pandas/issues/53706): `SeriesGroupBy.nlargest(as_index=False)` raised ValueError | Easy | Reproduce on `main`. If it now works, add a parametrized test in `pandas/tests/groupby/methods/test_nlargest_nsmallest.py` asserting the expected frame, and reference the issue in a comment. |
| [#57837](https://github.com/pandas-dev/pandas/issues/57837): `shift(freq=DateOffset(hours=1))` across DST raised NonExistentTimeError | Easy–Med | Rerun the Europe/Amsterdam example on `main` with `zoneinfo`. Add a test under `pandas/tests/series/methods/test_shift.py`. If it still fails, say so on the issue before trying a fix. |
| [#45800](https://github.com/pandas-dev/pandas/issues/45800): `df.agg([custom_func, "sum"])` → "cannot combine transform and aggregation" | Easy | Rerun the reproducer. Add a test in `pandas/tests/apply/test_frame_apply.py` covering a list mixing a user function and a string reducer. |
| [#42851](https://github.com/pandas-dev/pandas/issues/42851): `value_counts(dropna=False)` turns `pd.NA` into `np.nan` | Easy–Med | Check `pd.Series(["a","a","b",pd.NA]).value_counts(dropna=False).index` on `main`, for both object and `string` dtype. Pin the current correct behaviour in `pandas/tests/base/test_value_counts.py`. |

### 2. scikit-learn/scikit-learn (Tier 1, score 91.3)

**What it is.** The standard classical-ML library.
**Why it fits.** It matches your ML coursework exactly, and it has the most responsive triage in this sample (95% of recent issues answered). The documentation and contributor guide are the best of all 72. Classic `good first issue`/`Easy` issues get a PR within hours, so this report targets docs bugs and meta-issue sub-items instead.

**Setup**
```bash
git clone https://github.com/<you>/scikit-learn && cd scikit-learn
python -m venv .venv && source .venv/bin/activate
pip install numpy scipy cython meson-python ninja pytest pre-commit
pip install --editable . --verbose --no-build-isolation --config-settings editable-verbose=true
pytest sklearn/feature_selection -q
pre-commit install
```
**Workflow.** No CLA. Comment on the issue before starting; a bot warns against PRs on issues still marked `Needs Decision`/`Needs Triage`. User-facing changes need a changelog fragment in `doc/whats_new/upcoming_changes/<module>/<PR>.<type>.rst`. Expect at least 2 maintainer approvals. Maintainers have publicly pushed back on agent-generated reports (seen on #35143), so write in your own words.

| Issue | Difficulty | Attack plan |
|---|---|---|
| [#35033](https://github.com/scikit-learn/scikit-learn/issues/35033): RFECV `cv_results_` split ranking/support shapes documented without the feature axis | Easy | Confirm the shapes with a 3-fold RFECV. Fix the two `cv_results_` entries in the `RFECV` docstring in `sklearn/feature_selection/_rfe.py` to `(n_subsets_of_features, n_features)`. Note: another user asked to confirm the approach on 2026-09-25, so check the thread first. |
| [#33958](https://github.com/scikit-learn/scikit-learn/issues/33958): META: Use narwhals in codebase → "Add an entry for 'dataframe' in the glossary" | Easy | Comment that you are taking only the glossary sub-item. Add a `dataframe` term to `doc/glossary.rst` (pandas/polars/narwhals, `set_output`), cross-linking `array-like`. |
| [#33457](https://github.com/scikit-learn/scikit-learn/issues/33457): META: `sample_weight` support for all estimators | Medium | Pick ONE unclaimed estimator; `TransformedTargetRegressor` (forwarding only) is a good start. Comment first. Implement `fit(..., sample_weight=None)` and add an equivalence test (integer weights ≡ repeated rows) following `sklearn/utils/_test_common`. |
| [#31595](https://github.com/scikit-learn/scikit-learn/issues/31595): property + same-name attribute docstring renders wrong (`BaggingClassifier.estimators_samples_`) | Medium | Build the docs locally (`make -C doc html-noplot`). Reproduce. Propose ONE documentation convention (maintainers disagree, so get agreement first). Then add the consistency test in `sklearn/tests/test_docstrings.py` they asked for. Flagged "not a good *first* issue", so do it second. |

### 3. matplotlib/matplotlib (Tier 1, score 92.9 — highest)

**What it is.** The plotting library behind most of the PyData stack.
**Why it fits.** It has the highest score in the sample: 71% merge rate, 93% of issues answered, 64 new authors in 6 months. Its `first-contribution` label is applied automatically to newcomers' issues. It is a **GSoC 2026 sub-org under NumFOCUS**, a direct path to GSoC 2027.

**Setup**
```bash
git clone https://github.com/<you>/matplotlib && cd matplotlib
python -m venv .venv && source .venv/bin/activate
python -m pip install -r requirements/dev/dev-requirements.txt
python -m pip install --verbose --no-build-isolation --editable .
pytest lib/matplotlib/tests/test_patches.py -q
pre-commit install
```
**Workflow.** No CLA. Docs PRs need a docs build (`cd doc && make html-noplot`). New features need a `doc/release/next_whats_new/` entry and API changes a `next_api_changes` entry. Matplotlib runs regular new-contributor meetings and an "incubator" chat room for first-timers.

| Issue | Difficulty | Attack plan |
|---|---|---|
| [#30165](https://github.com/matplotlib/matplotlib/issues/30165): `Ellipse.get_vertices` + `get_transform` applies patch transform twice | Easy | Do what maintainer rcomer proposed: clarify the `Patch.get_transform` docstring in `lib/matplotlib/patches.py` and cross-reference `get_data_transform`. Optionally add a sentence to `get_vertices`. Docs-only PR. |
| [#28745](https://github.com/matplotlib/matplotlib/issues/28745): document `linestyle` as an `:mpltype:` | Easy–Med | Add a linestyle table to `_docstring.interpd`, register the link target in `lib/matplotlib/sphinxext/roles.py`, and replace hand-written linestyle descriptions. Someone asked in March 2026 but never followed up, so comment before you start. |
| [#31809](https://github.com/matplotlib/matplotlib/issues/31809): ipympl: `figure.canvas.manager` becomes None after repeated `hist` calls | Medium | Install `ipympl` + Jupyter and reproduce the exact sequence. Instrument `pyplot._pylab_helpers.Gcf` (destroy/del paths) to find who de-registers the figure. tacaswell's hint: check `type(fig.canvas)` before and after. Post findings before coding. |

### 4. secdev/scapy (Tier 1, score 84.6 — best networking fit)

**What it is.** A Python program/library to forge, send, sniff and dissect packets. It is the standard tool for protocol testing and security research.
**Why it fits.** It turns your Computer Networks course into code: new protocol layers are a Python class with a `fields_desc` list, and tests are text files. It is pure Python with a 71% merge rate, 92% of issues answered, and a small team that knows contributors by name.

**Setup**
```bash
git clone https://github.com/<you>/scapy && cd scapy
python -m venv .venv && source .venv/bin/activate
pip install -e . tox
cd test && ./run_tests -t regression.uts -K needs_root   # UTscapy test campaigns (*.uts)
tox -e flake8,mypy                                        # lint + types, as in CI
```
**Workflow.** No CLA. New files need SPDX license headers. Tests are `.uts` campaign files (`test/scapy/layers/*.uts`, `test/contrib/*.uts`). Use draft PRs early. **They refuse purely AI-generated reports and AI-written replies to maintainers**, and they publish an `AGENTS.md` describing the rules for tooling.

| Issue | Difficulty | Attack plan |
|---|---|---|
| [#3625](https://github.com/secdev/scapy/issues/3625): add PIM Register / Register-Stop messages to `scapy.contrib.pim` | Easy–Med | Read RFC 7761 §4.9.3–4.9.4. Add `PIMv2Register` (B/N bits + encapsulated packet) and `PIMv2RegisterStop` (encoded group/source) classes with `bind_layers` on the PIM type. Add build/dissect tests from a pcap or hand-written bytes in `test/contrib/pim.uts`. |
| [#4680](https://github.com/secdev/scapy/issues/4680): configurable snapshot length and buffer size for pcap sockets | Medium | Find where `L2pcapListenSocket`/libpcap handles are opened (`scapy/arch/libpcap.py`). Plumb new `snaplen`/`buffer_size` kwargs to `pcap_set_snaplen`/`pcap_set_buffer_size` before `pcap_activate`. Default to current behaviour. Add a test that checks the values are passed through. |
| [#4055](https://github.com/secdev/scapy/issues/4055): unify network-configuration reloading | Easy–Med | Opened by a maintainer. Add `conf.network_resync()` (or similar) that calls the existing route resync and iface reload. Keep the old names as deprecated aliases. Test in `test/regression.uts`. Propose the name in a comment first. |
| [#3670](https://github.com/secdev/scapy/issues/3670): `MultipleTypeField` wrapping `FieldListField` doesn't build | Medium–Hard | Write a minimal failing `.uts` test from the report. Trace `Packet.getfieldval` → `MultipleTypeField._find_fld_pkt_val` during build without `explicit=1`. Fix the fallback-to-default path. Check #1143 for the related history. |

### 5. pydata/xarray (Tier 1, score 83.0)

**What it is.** N-dimensional labelled arrays and datasets, the pandas of NetCDF/Zarr/climate/ML-on-tensors data.
**Why it fits.** It is a natural step after pandas and NumPy. It uses its own `contrib-good-first-issue` label, and collaborator headtr1ck is actively triaging and writing fix plans for newcomers (October 2026).

**Setup**
```bash
git clone https://github.com/<you>/xarray && cd xarray
mamba env create -f ci/requirements/environment.yml && mamba activate xarray-tests
pip install -e . --no-deps
pytest xarray/tests/test_dataarray.py -q
pre-commit install
```
**Workflow.** No CLA. Every user-visible change gets a `doc/whats-new.rst` entry. Deprecations follow strict warning-category rules: `FutureWarning` for users.

| Issue | Difficulty | Attack plan |
|---|---|---|
| [#11721](https://github.com/pydata/xarray/issues/11721): Tracking: open deprecations | Easy | Opened 2026-10-13. Comment to claim ONE unclaimed item, e.g. "remove `.dt.weekofyear`/`.week`", "remove `quantile(interpolation=…)`" or "`reset_encoding` uses `UserWarning` and cites the wrong version". Remove the code, update tests that expected the warning, and add a whats-new "Breaking changes" line. |
| [#10591](https://github.com/pydata/xarray/issues/10591): structured-dtype `_FillValue` + Zarr v2 → TypeError | Medium | Follow headtr1ck's 2026-10-12 plan: skip masking for structured dtypes in `CFMaskCoder.decode` (`xarray/coding/variables.py`), keep the fill value in encoding, and add a round-trip test in `xarray/tests/backends/test_zarr.py`. He asked the reporter first, so post a comment before coding. |
| [#8239](https://github.com/pydata/xarray/issues/8239): address repo-review suggestions | Easy | One small PR per item, as maintainers asked. Good candidates: `PC170` (add pygrep-hooks) or `MY104` (`enable_error_code = ["ignore-without-code"]` in mypy config, then fix the fallout). Skip `PC901` (someone asked for it). |

### 6. numpy/numpy (Tier 1, score 79.3)

**What it is.** N-dimensional arrays; everything else in this report is built on it.
**Why it fits.** It has a 71% merge rate, 104 new authors in 6 months, and a 2026 "Sustaining Open Source" project that explicitly marks issues "Looking for Contributor". Code changes need C knowledge, but the docs issues below don't.

**Setup**
```bash
git clone --recurse-submodules https://github.com/<you>/numpy && cd numpy
python -m venv .venv && source .venv/bin/activate
pip install -r requirements/all_requirements.txt   # includes spin, meson, pytest
spin build && spin test -v -- numpy/_core/tests/test_multiarray.py
spin docs                                           # for doc PRs
```
**Workflow.** No CLA. Use commit-message prefixes (`DOC:`, `BUG:`, `MAINT:`, `ENH:`). There is an AI/automated-contribution policy; one claimant withdrew from #32272 citing it.

| Issue | Difficulty | Attack plan |
|---|---|---|
| [#28028](https://github.com/numpy/numpy/issues/28028): document IDE debugging setup | Easy | Member InessaPawson (2026-10-04) asked for a short "Debugging from an IDE" subsection in `doc/source/dev/development_environment.rst`. Set up VS Code + `spin gdb`/lldb yourself, write down exactly what worked (launch.json, debug build flags), and link the existing advanced-debugging page. |
| [#28311](https://github.com/numpy/numpy/issues/28311): missing descriptions for `ndarray.__abs__` and `ndarray.__new__` | Easy | Only these two are unclaimed; `__int__`/`__float__`/`__complex__` are claimed. Add docstrings via `add_newdoc` in `numpy/_core/_add_newdocs.py`, following neighbouring dunder entries. Build docs to check the reference page. |
| [#32272](https://github.com/numpy/numpy/issues/32272): `.npz` zip container unspecified | Easy–Med | ngoldbaum OK'd a docs PR describing *current* behaviour (deflate vs stored, zip64, member naming/order, timestamps, not byte-reproducible). Two people offered in August without a PR; ask on the issue whether it's free first. |

### 7. pylint-dev/pylint (Tier 1, score 77.5)

**What it is.** A static analyser for Python, built on the `astroid` inference engine.
**Why it fits.** Each bug teaches you how Python code is parsed and inferred. Issues are crisply triaged (`Needs PR` = accepted and specified, ready to implement), and the merge rate is 74%. The test format is simple: a functional test is a `.py` file plus a `.txt` of expected messages.

**Setup**
```bash
git clone https://github.com/<you>/pylint && cd pylint
python -m venv .venv && source .venv/bin/activate
pip install -e . -r requirements_test.txt
pytest tests/test_functional.py -k using_constant_test -q
pre-commit install
```
**Workflow.** No CLA. Every PR needs a news fragment in `doc/whatsnew/fragments/<issue>.<type>` (`false_positive`, `new_check`, …). Functional tests live in `tests/functional/<letter>/`. Some fixes belong in `pylint-dev/astroid` instead, which is a separate repo.

| Issue | Difficulty | Attack plan |
|---|---|---|
| [#9556](https://github.com/pylint-dev/pylint/issues/9556): `using-constant-test` false positive on `if e.code:` for a caught `SystemExit` | Easy–Med | Add the reproducer as a functional test. Check what astroid infers for `SystemExit().code`; it probably infers the class attribute default. Fix in the checker (skip when inference yields an instance-attribute default) or in astroid's exception brain. |
| [#10630](https://github.com/pylint-dev/pylint/issues/10630): extend W0102 `dangerous-default-value` to `os.getenv`/`os.environ` | Medium | Pierre-Sassoulas: an implementer must also fix #4659, so read it first. Extend the call-detection in `pylint/checkers/base/basic_checker.py`, then add functional tests and a `new_check`/`other` fragment. |
| [#9977](https://github.com/pylint-dev/pylint/issues/9977): `wrong-import-order`/`ungrouped-imports` ignore isort custom sections | Medium | In `pylint/checkers/imports.py`, see how `isort.place_module` results are bucketed (only stdlib/third/first-party). Map custom `known_<section>`/`sections` from the isort config into extra ordered buckets. Add tests with a temporary isort config. |

### 8. netbox-community/netbox (Tier 1, score 71.6 — networking)

**What it is.** The leading open-source network source of truth (IPAM, DCIM, cabling, circuits), built on Django and PostgreSQL. Network engineers use it every day.
**Why it fits.** It applies network domain knowledge to a big Python codebase. It has the best merge rate in the sample (92%) because work is **assigned before PRs** (`status: needs owner` → you volunteer → a maintainer assigns you). So once you have the issue, your PR is expected.

**Setup**
```bash
sudo apt install postgresql redis-server
git clone https://github.com/<you>/netbox && cd netbox
python -m venv .venv && source .venv/bin/activate && pip install -r requirements.txt
cp netbox/netbox/configuration_example.py netbox/netbox/configuration.py   # set DATABASE, REDIS, SECRET_KEY
python netbox/manage.py migrate && python netbox/manage.py runserver
python netbox/manage.py test dcim --keepdb
```
**Workflow.** Comment to volunteer, wait for assignment, and **do not open a PR before you are assigned** (it will be closed). Branch from `main` for features/bugs. They do not accept AI-generated issues.

| Issue | Difficulty | Attack plan |
|---|---|---|
| [#23412](https://github.com/netbox-community/netbox/issues/23412): show `API_TOKEN_PEPPERS` example as a raw string | Easy | Volunteer. Update the example in `docs/configuration/required-parameters.md` (or wherever `API_TOKEN_PEPPERS` is documented) to use `r'...'`, and add a one-line warning about backslashes. Optionally do the same in `configuration_example.py`. |
| [#23248](https://github.com/netbox-community/netbox/issues/23248): show object tags in global search results | Easy–Med (`complexity: low`) | Volunteer. Find the search results table (`netbox/netbox/tables/` + `search/`). Add a `TagColumn` that renders the object's tags, prefetching tags to avoid N+1 queries. Add a view test. |
| [#23237](https://github.com/netbox-community/netbox/issues/23237): related objects of object-type custom fields aren't clickable | Medium | Volunteer. Find the template/panel rendering "Related Objects" for custom fields (`extras/`). Link each count to the related model's list view filtered by `cf_<name>=<pk>`. Add a test. |

### 9. sktime/sktime (Tier 2, score 83.7)

**What it is.** A unified scikit-learn-style framework for time-series forecasting, classification and detection.
**Why it fits.** It is the most welcoming project in this sample, with mentoring, Discord dev chat and very generous GFI labelling. That popularity is also the catch: 80 open GFIs without a linked PR, but most have several "can I work on this" comments. Pick a concrete sub-task, say exactly what you'll do, and open a draft PR quickly. fkiraly accepts parallel work if you credit others.

**Setup**: `pip install -e ".[dev]"`, `pytest sktime/forecasting/tests -q`, and `pre-commit install`. Use `check_estimator(MyEstimator)` for estimator PRs. No CLA. The all-contributors bot credits non-code work too.

| Issue | Difficulty | Attack plan |
|---|---|---|
| [#8384](https://github.com/sktime/sktime/issues/8384): weekly CI job against nightly numpy/pandas/sklearn wheels | Easy–Med | Two people asked (Jun 2025, Jan 2026) but there is no PR. Ask whether it's free. Write `.github/workflows/nightly-deps.yml` (cron weekly) that installs from the scientific-python nightly index (SPEC 4) and runs the core test subset fkiraly named. |
| [#11329](https://github.com/sktime/sktime/issues/11329): global recursive reduction forecasters are 60–400× slower than mlforecast | Medium | Others are on the `get_expected_pred_idx` part, so take a *different* bottleneck, e.g. per-step `WindowSummarizer` rebuilds or repeated mtype conversion. Add the benchmark script fkiraly asked for and report before/after numbers. |
| [#7285](https://github.com/sktime/sktime/issues/7285): move detection-notebook plotting functions into `detection.plotting` | Easy–Med | PR #7300 is stalled with failing tests; fkiraly said to "start fresh or build on #7300, credit sank8-2". Rebase the functions, give each an `ax=None` param, add smoke tests, and update the notebook to import them. |

### 10. pytorch/pytorch (Tier 2, score 76.2)

**What it is.** The deep-learning framework.
**Why it fits.** It is the core of your ML stack, and `torch.distributed` collective bugs mix DL with networking (process groups, gloo/TCP). It's huge and slow-moving for outsiders, but well run: unassigned plus `triaged` explicitly means "we want community help".

**Setup** (Python-only changes; no 2-hour C++ build):
```bash
git clone https://github.com/<you>/pytorch && cd pytorch
python -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt
./tools/nightly.py checkout -b my-fix -p .venv   # installs nightly binaries, links your Python sources
python test/distributed/test_c10d_gloo.py -k scatter -v
pip install lintrunner && lintrunner init && lintrunner -a
```
**Workflow.** **Sign the Linux Foundation EasyCLA** (the bot prompts on your first PR). CI is huge; a maintainer must approve workflows for first-timers. Land via `@pytorchbot merge` after approval.

| Issue | Difficulty | Attack plan |
|---|---|---|
| [#74602](https://github.com/pytorch/pytorch/issues/74602): `scatter_object_list` uses group-local `my_rank` against a global `src` on subgroups | Medium | Reproduce with a 4-process gloo CPU run (`torch.multiprocessing.spawn`) and a subgroup `[1,2]`. In `torch/distributed/distributed_c10d.py`, compare against `get_rank()` / translate `src` with `get_group_rank` (newer APIs may already provide `group_src`). Add a test in `test/distributed/test_c10d_gloo.py`. |
| [#89197](https://github.com/pytorch/pytorch/issues/89197): bool tensors in gloo collectives | Easy (tests only) | Reportedly fixed by #105354 (2026-10-04 comment), and the reporter offered a test-only PR. Only take it if they don't follow up within ~2 weeks. Then add `test_bool_tensors_*` for all_reduce/all_gather/gather/scatter. |
| [#74809](https://github.com/pytorch/pytorch/issues/74809): `gather` of a tensor built from a permuted NumPy array | Easy (tests only) | Same situation: it appears fixed and a regression test is proposed (`test_gather_permuted_input`). Coordinate on the thread instead of racing. |

### 11. pytest-dev/pytest (Tier 2, score 75.1)

**What it is.** The testing framework you will use in every other project here.
**Why it fits.** Small, well-scoped docs PRs give you a merged contribution in a high-profile repo, and you learn the changelog-fragment workflow that many projects copy.

**Setup**: `pip install -e . tox`, `tox -e py`, or `pytest testing/test_runner.py -q`. Docs: `tox -e docs`. No CLA. Every PR needs `changelog/<issue>.<type>.rst` and your name added to `AUTHORS`. **Policy: purely agentic PRs are not accepted**; AI-assisted is fine with real human review.

| Issue | Difficulty | Attack plan |
|---|---|---|
| [#12235](https://github.com/pytest-dev/pytest/issues/12235): explain why `OutcomeException` subclasses `BaseException` | Easy | Maintainers rejected changing the base class and want a docstring explaining it. Edit `src/_pytest/outcomes.py`: catching `Exception` in user code must not swallow skip/fail/xfail. Add a `changelog/12235.doc.rst`. |
| [#8208](https://github.com/pytest-dev/pytest/issues/8208): docs example of a plugin managing a resource + parametrized fixtures | Easy–Med | Write a small `conftest.py` example (resource opened in `pytest_configure`, closed in `pytest_unconfigure`, parametrized fixture using it) under `doc/en/example/`, and link it from the fixtures how-to. The 2021 volunteer vanished; comment first. |
| [#13266](https://github.com/pytest-dev/pytest/issues/13266): docs LaTeX/PDF build failing (sphinx inkscape converter) | Easy–Med (verify first) | Run `tox -e docs` plus a LaTeX build locally. If upstream sphinx#13403 fixed it, report that so the issue can be closed (triage counts!). Otherwise pin or patch the converter config in `doc/en/conf.py`. |

### 12. mitmproxy/mitmproxy (Tier 2, score 70.5 — networking)

**What it is.** An interactive HTTPS-intercepting proxy (console, web UI and `mitmdump`) with a Python addon API.
**Why it fits.** It's the deepest networking codebase here (asyncio, TLS, HTTP/1-2-3, WireGuard modes) and the core is pure Python. Fewer beginner labels, so you'll be working on real features.

**Setup**: `pip install -e ".[dev]"` (or `uv sync`), `pytest test/mitmproxy/proxy -q`, `tox -e lint`. No CLA. Add a `CHANGELOG.md` line.

| Issue | Difficulty | Attack plan |
|---|---|---|
| [#8088](https://github.com/mitmproxy/mitmproxy/issues/8088): Happy Eyeballs (RFC 8305) for upstream connections | Medium | In `mitmproxy/proxy/server.py` (~L211), pass `happy_eyeballs_delay=0.25` (maybe an option) to `asyncio.open_connection`. Reproduce with a host whose AAAA record is unreachable. Add a test with a mocked resolver. Ask on the issue whether it should be an option. |
| [#8012](https://github.com/mitmproxy/mitmproxy/issues/8012): delete all flows matching a filter | Easy–Med | Check whether a core command already exists (`view.flows.remove`, `view.flows.resolve <filter>`). If not, add `view.flows.remove_matching <filter>` using `mitmproxy.flowfilter`, plus a test. A UI button can follow as a separate PR. |
| [#6888](https://github.com/mitmproxy/mitmproxy/issues/6888): Windows MSI upgrade leaves stale "Installed apps" entries | Medium (needs Windows) | Reproduce by installing 2 versions in a Windows VM. Inspect the installer config under `release/` (upgrade code / "uninstall previous version" setting). Fix so upgrades replace the old entry. |

### 13. mlflow/mlflow (Tier 2, score 88.8)

**What it is.** The open-source MLOps / GenAI-ops platform: tracking, model registry, evaluation, tracing.
**Why it fits.** Its score is #4 overall (64% merge rate, 212 new authors in 6 months), and it's very relevant to ML jobs. **But every currently open `good first issue` already has a linked PR**; new ones are claimed within hours. Strategy: watch the label (GitHub "Watch → Custom → Issues") and claim new ones the day they appear, or take the bigger `help wanted` items below.

**Setup**: `dev/dev-env-setup.sh -d .venv` (or `uv sync`), `pytest tests/tracking -q`, `pre-commit install`. **DCO required: commit with `git commit -s`.** Maintainers apply `rn/*` release-note labels.

| Issue | Difficulty | Attack plan |
|---|---|---|
| [#11484](https://github.com/mlflow/mlflow/issues/11484): pass Docker build args / secrets to `mlflow models build-docker` | Medium | First, post a short design comment choosing between `--build-arg` and BuildKit `--secret` (security trade-offs are already discussed in the thread). Then add a CLI option in `mlflow/models/cli.py` → docker utils, with tests mocking `docker build`. |
| [#14841](https://github.com/mlflow/mlflow/issues/14841): bookmarkable URL with the "Search Experiments" filter pre-filled | Easy–Med (TypeScript) | In `mlflow/server/js`, sync the experiment-search box with a URL query parameter (read on mount, write on change). Add a Jest test. A volunteer from March 2025 never followed up; comment first. |

Only 2 issues are listed because no third one passed verification on 2026-10-16.

### 14. marimo-team/marimo (Tier 3, score 80.4)

**What it is.** A reactive Python notebook stored as plain `.py` files, a modern Jupyter alternative.
**Why it fits.** It has a 78% merge rate and fast maintainers, and you'd use it for ML experiments. **However, the free issues are mostly frontend (TypeScript/React).** Choose it only if you want to learn TS.

**Setup**: `make fe && make py` (pnpm + uv/hatch), `make test`. **CLA required:** comment `I have read the CLA Document and I hereby sign the CLA` on your first PR.

| Issue | Difficulty | Attack plan |
|---|---|---|
| [#2918](https://github.com/marimo-team/marimo/issues/2918): config option to hide/disable notebook download options | Easy–Med (Python + TS) | Add a key under `[tool.marimo]`/user config (Python `_config` schema), send it to the frontend, and hide menu items in the notebook menu component. Add a config test plus a frontend unit test. |
| [#2405](https://github.com/marimo-team/marimo/issues/2405): "Copy as Altair chart" for `mo.ui.data_explorer` | Medium (TS) | Per mscolnick: generate the Altair Python snippet in the frontend from the explorer's encoding state, add a copy button, and write thorough unit tests for the string generator. |
| [#3825](https://github.com/marimo-team/marimo/issues/3825): persist collapsed markdown-heading state | Medium (TS) | Contested: a contributor is waiting on scope since Jul 2026. Only pick it up if a maintainer confirms the localStorage-first approach and the previous volunteer doesn't reply. |

---

## Honorable mentions (high score, but no free issues today)

| Repo | Score | Why not shortlisted (2026-10-16) | When to come back |
|---|---|---|---|
| zulip/zulip | 88.3 | Help-wanted issues have 20–47 comments; only 2 without PRs and both are actively claimed. Work is mostly TypeScript/frontend. | Great **GSoC** org (Zulip runs its own). Use `@zulipbot claim` and read their "first contributions" guide. Best if you want to learn Django + TS. |
| huggingface/transformers | 86.6 | Remaining Good First/Second issues are 2023–2025, stale or out of scope. **No-code-agent policy for first-time contributors.** | Watch for model-addition and docs issues. Fixing a bug you hit in your own ML project is the best entry. |
| unit8co/darts | 79.8 | Fresh GFIs have PRs; only #1278 is free and it was claimed the day before this check. | Small team, 75% merge rate: good once a new GFI appears. |
| huggingface/peft | 79.0 | Reporters fix their own bugs, and PRs need a maintainer `/peft-triage approved` first. | Excellent responsiveness (87%). Contribute when you find a bug while fine-tuning. |
| pymc-devs/pymc | 77.0 | `beginner friendly` items without PRs date from 2021–2023; PyMC doesn't assign (a PR is the claim). | **GSoC 2026 org** (NumFOCUS): watch the GSoC 2027 ideas list. |
| jax-ml/jax | 74.0 | The GFI [#20293](https://github.com/jax-ml/jax/issues/20293) has 10+ "can I take this?" comments and an old PR. A textbook example of what not to do. | Contribute via bugs you hit; good documentation issues appear occasionally. |
| astral-sh/ruff | 74.3 | Rust codebase. | If you learn Rust: very fast, friendly reviews (66% merge, 1,815 merged PRs in 4 months). |
| narwhals-dev/narwhals | 72.0 | 80% merge rate and lovely maintainers, but fresh bugs are claimed within days (often agent-driven). | Great second project after pandas; check new issues daily. |

The full ranking of all 72 repos is in `candidates.csv`.

---

## 4-week starter roadmap

Budget about 8–10 hours a week next to coursework.

| Week | Goal | Concrete steps | Done when |
|---|---|---|---|
| **1: Set up and observe** | Toolchain plus 2 repos building locally | Set up git (`user.name/email`, SSH key, signed commits optional). Fork and build **pandas** and **scapy** (or **scikit-learn**) from source using the commands above, and run their test subsets. Read both CONTRIBUTING guides and AI policies. Lurk on 3–4 recently merged PRs to see review style. Join the pandas Slack / sktime Discord / matplotlib incubator chat. | Both repos build; you can run one test file in each |
| **2: First PR (test/docs)** | One merged-quality small PR | `/take` a pandas `Needs Tests` issue ([#53706](https://github.com/pandas-dev/pandas/issues/53706) or [#45800](https://github.com/pandas-dev/pandas/issues/45800)). Write the test, run pre-commit, open the PR with "closes #…". In parallel, pick a docs issue (numpy [#28311](https://github.com/numpy/numpy/issues/28311) or pytest [#12235](https://github.com/pytest-dev/pytest/issues/12235)). | 1–2 PRs open, CI green |
| **3: First real fix** | A bug or feature in code you understand | Respond to review feedback within 48 h. Start one medium issue: scapy [#3625](https://github.com/secdev/scapy/issues/3625) (protocol layer), pylint [#9556](https://github.com/pylint-dev/pylint/issues/9556), or xarray [#11721](https://github.com/pydata/xarray/issues/11721) item. Comment your plan first and wait for a nod if the issue says "needs decision". | Plan acknowledged by a maintainer; draft PR up |
| **4: Consolidate and pick a home** | Choose 1 "home" project for the next 3–6 months | Land or iterate the week-3 PR. Help triage: reproduce 2–3 new issues in your home project and post minimal reproducers (maintainers value this a lot). Write a short blog/LinkedIn post about what you learned. If you're targeting GSoC 2027, list your home project's past ideas and open a discussion. | 2+ merged PRs; 1 home project chosen |

**Choosing the home project.** Pick ML → scikit-learn, pandas or xarray. Pick networking → scapy, netbox or mitmproxy. Pick GSoC-oriented → matplotlib or pymc (both GSoC 2026 orgs under NumFOCUS), or zulip.

---

## PR/issue etiquette (2026 edition)

1. **Read CONTRIBUTING *and* the AI policy before writing code.** pandas, pytest, numpy, scikit-learn, scapy, netbox and transformers all have explicit rules in 2026: disclose AI use, no purely agentic PRs, and no AI-written issues or replies. Violations get PRs closed and accounts blocked; this was seen several times while verifying issues.
2. **Claim the way the project expects:**
   - pandas: `/take` (max 2 at a time).
   - netbox: volunteer, then wait to be assigned.
   - zulip: `@zulipbot claim`.
   - pymc: open a PR (no assignment).
   - peft: wait for `/peft-triage approved`.
   - everyone else: one comment stating *your plan*, not "can I work on this?".
3. **Never post "is this still open? can I take it?" without a plan.** jax #20293 has 10+ such comments and no progress. Instead, write: "I reproduced this on main (output …). I plan to change X in `file.py` and add a test in Y. OK?"
4. **Check for existing work before starting:** linked PRs, PRs that *mention* the issue (GitHub search `is:pr <issue-number>`), and fork commits in the timeline. Several "good" issues in this report's candidate list were silently taken that way.
5. **Keep PRs small and single-purpose.** Run the project's linters and tests locally first, and fill in the PR template completely, linking the issue (`Closes #123`).
6. **Respond to reviews within a couple of days.** If you can't, say so, or release the issue (`/untake`). Stale claims block others.
7. **Don't ping maintainers repeatedly.** One polite bump after a week of silence is fine.
8. **Write the PR description yourself, in plain words:** what changed, why, how you tested it. Paste exact commands and output.
9. **Credit others** when you build on a stalled PR (sktime explicitly asks for this).
10. **Non-code contributions count:** reproducing bugs, closing obsolete issues with evidence (e.g. "fixed on main by #105354"), and improving docs. They build reputation fast.

---

## Programs and deadlines

Official program pages could not be fetched from this environment, so the dates below come from search results and the NumFOCUS repo, current as of 2026-10-16. **Confirm on the official sites before relying on them.**

| Program | What | Known/expected dates | Targets for you |
|---|---|---|---|
| **Google Summer of Code 2027** | Paid 12+ week remote project with an open-source org; students and beginners eligible | **2027 dates not published yet.** For reference, GSoC 2026 ran: org applications Jan 19 – Feb 3, accepted orgs announced Feb 19, **contributor applications Mar 16 – Mar 31**, results Apr 30, coding from May 25 ([GSoC timeline](https://developers.google.com/open-source/gsoc/timeline)). Expect a similar 2027 schedule: start contributing **now**, shortlist orgs when they're announced (~mid-Feb 2027), and submit proposals in March. | **NumFOCUS** umbrella (2026 sub-orgs included **Matplotlib, PyMC, PyTorch-Ignite**, ArviZ, HoloViz, CVXPY, sbi, scikit-bio, Stan… ([numfocus/gsoc](https://github.com/numfocus/gsoc))). **Python Software Foundation** umbrella (many Python projects). **Zulip**, **SymPy**, **DeepChem**, **Mesa**, **OpenAstronomy** (regulars). Check whether scikit-learn/pandas/sktime apply in 2027. NumFOCUS requires you to cite AI tools in proposals. |
| **Hacktoberfest 2026** | Month-long open-source event | **Oct 1–31, 2026 (happening now).** Third-party write-ups report that **2026 changed the rules: PRs/MRs no longer count toward rewards** (an anti-spam measure). Progress is reportedly tracked via activity-based virtual stickers, run by MLH + DEV with DigitalOcean, with an open-source-AI focus. For comparison, 2025 needed 6 accepted PRs ([2025 guide](https://www.digitalocean.com/blog/how-to-participate-hacktoberfest-2025), [2026 changes](https://spiketoday.com/hacktoberfest-rules/), [overview](https://openresource.dev/articles/hacktoberfest/)). Verify on hacktoberfest.com. | Treat it as motivation, not a target. **Never spam PRs.** Maintainers in this report actively block low-effort contributions. |
| **LFX Mentorship** (Linux Foundation, CNCF, etc.) | Paid 12-week mentorships; 3 terms per year | Typical schedule: **Spring Mar 1–May 31** (projects listed ~mid-January, applications open ~4 weeks); Summer Jun–Aug; Fall Sep–Nov. So the **Spring 2027 application window is likely ~mid-January to mid-February 2027** ([LFX timelines](https://docs.linuxfoundation.org/lfx/mentorship/mentorship-program-timelines), [LFN timeline](https://lf-networking.atlassian.net/wiki/spaces/LN/pages/15673598)). | Networking-heavy LF projects fit your CN background: **LF Networking** (ONAP, OpenDaylight, Nephio), **CNCF** (Kubernetes networking, Cilium, Envoy), **PyTorch Foundation** projects. Apply on mentorship.lfx.linuxfoundation.org. |
| **MLH Fellowship (Open Source track)** | 12-week paid, remote fellowship working on open-source projects in small pods | Rolling batches every few months; apply early, since each batch has its own deadline ([MLH Fellowship](https://fellowship.mlh.io/programs/open-source), [overview](https://www.profellow.com/?p=104187)). | Apply with your merged PRs from this plan as evidence. |
| **Others worth knowing** | | Outreachy (Dec–Mar and May–Aug cohorts; check eligibility), **sktime's own mentoring programme**, **C4GT** (India, summer). | |

**Suggested timeline to GSoC 2027.** October–December 2026: land 3–5 PRs in one home project, ideally a 2026 GSoC org. January 2027: talk to mentors about project ideas on the org's public channels. February 2027: check the accepted-org list. March 2027: submit 1–2 strong proposals. Apply to LFX Spring 2027 in parallel in January–February.

---

## Reproducing / updating this report

```bash
cd oss-contributions/data
python3 git_activity.py          # commit activity via treeless git clones (resumable)
# search_stats.csv / responsiveness.csv were filled from GitHub search API queries (see REPORT method)
python3 score.py                 # -> ../candidates.csv
```
Per-issue verification notes, including every *excluded* issue and why, are in `data/raw/verified.md`.
