# Per-issue verification notes (github.com issue page, 2026-10-16)
## scapy
- 4680 OK: open, unassigned, no PRs. Ask: configurable snaplen + buffer size for pcap (libpcap) sockets; author notes TPACKET_V2 ring benefits. Maintainer gpotter2 labeled enhancement Nov 2025.
- 3625 OK: open, unassigned, no PRs, no comments. Add PIM Register / Register-Stop (RFC 7761 4.9) to scapy/contrib/pim.py; author sketched fields (border, null bits; group, source).
- 3670 OK: open, unassigned, no PRs. MultipleTypeField wrapping FieldListField fails to build unless packet built from bytes; getfieldval returns first element; workaround explicit=1; maybe related #1143.
- 4513 EXCLUDED: open PR #5117 references it.
- 4343 EXCLUDED: open PR #4950 references it.
- 4739 EXCLUDED: claimed by alxroyer-thales (fork commits).
- 4055 OK: open, unassigned, no PRs/comments. Unify route resync + iface reload names; add conf network_resync() calling both (author: guedou, maintainer).
## matplotlib
- 32246 EXCLUDED: design discussion ongoing; author rokokol has PR #32252 + follow-up planned.
- 31378 EXCLUDED: PR #31365 covers it.
- 32328 EXCLUDED: likely fixed by #31977 (lands 3.12); needs-clarification.
- 30165 OK: open, unassigned, no PRs. Ellipse.get_vertices + get_transform applies patch transform twice; rcomer (member) proposes clarifying Patch.get_transform docstring + cross-ref get_data_transform. Small docs PR.
- 28745 OK: open, unassigned, no PRs. Document linestyle as :mpltype: (table in _docstring.interpd + linestyles reference page + roles.py link target). One contributor asked in Mar 2026 with no follow-up -> comment first.
- 31809 OK: open, unassigned, no PRs. ipympl: figure.canvas.manager becomes None after second figure + several ax.hist calls; tacaswell says figure is being de-registered from pyplot. Needs repro + root-cause.
- 29928 EXCLUDED: turned into backend bug #30898; mitra-gayathri active.
- 31147 EXCLUDED: SAI-HARISH2007 has fork commit.
## scikit-learn
- 35033 OK: open, unassigned, no PRs. RFECV cv_results_ split(k)_ranking/support shapes documented as (n_subsets,) but are (n_subsets, n_features); fix docstring in sklearn/feature_selection/_rfe.py. One contributor (RugvedBane) asked maintainers to confirm approach Sep 25 -> check before starting.
- 31595 OK: open, unassigned, no PRs. Property + same-name fitted attribute docstring renders wrong (BaggingClassifier.estimators_samples_, RF.feature_importances_, Pipeline.n_features_in_); maintainers want one consistent style + a test in test_docstrings.py. adrinjalali: "not a good first issue" (medium).
- 34807 EXCLUDED: PR #34899 open.
- 32661 SKIPPED: maintainer-owned process doc.
- 29521 EXCLUDED: open PR #34244 + several fork attempts.
- 34366 SKIPPED: maintainer-led accessibility project (CSS/theme), not a first task.
## pandas
- 53706 OK: open, unassigned, no PRs. jbrockmendel added Needs Tests 2026-10-14 (=> behaviour likely fixed on main; add regression test). groupby(as_index=False)['B'].nlargest() raised ValueError.
- 57837 OK: open, unassigned, no PRs. Needs Tests added 2026-10-14. Series.shift(freq=DateOffset(hours=1)) across Europe/Amsterdam DST raised NonExistentTimeError; check on main, add test in pandas/tests/tseries/offsets or series/methods/test_shift.py.
- 45800 OK: open, unassigned, no PRs; Needs Tests added 2026-10-14. df.agg([custom_func,"sum"]) raised "cannot combine transform and aggregation operations".
- 42851 OK: open, unassigned, no PRs; Needs Tests added 2026-10-14. value_counts(dropna=False) turned pd.NA into np.nan in result index.
(sklearn) 35143 EXCLUDED: lesteve: very low priority, unlikely to be reviewed; warned about agent-generated reports (sklearn automated-contribution policy).
- 33457 OK (meta): open, unassigned; tracks 51 estimators lacking sample_weight in fit. Only NearestCentroid claimed (prem-479); PRs #33588/#33477 cover some items. Pick ONE unclaimed estimator, comment first.
- 33958 OK (meta): open, unassigned, no linked PRs; unchecked TODOs incl. "Add an entry for 'dataframe' in the glossary" and "extend user guide on data interoperability" -> doc-sized first tasks.
## mlflow (note: every 'good first issue' currently has a linked PR -> they get claimed within hours)
- 11484 OK: open, unassigned, no PRs. Pass docker build args/secrets into `mlflow models build-docker` / `mlflow sagemaker build-and-push-container`. WeichenXu123: "makes sense"; community proposed multi-stage build + BuildKit secret mount; no maintainer design decision yet -> propose design first. endesapt offered Dec 2025 (stale).
- 11936 EXCLUDED: comment Oct 9 2026 says feature already exists on master (#12026, #14541).
- 14841 OK: open, unassigned, no PRs. Bookmark/share UI view with Search Experiments filter populated via URL query param (React frontend in mlflow/server/js). joelrobin18 asked Mar 2025, never followed up -> comment first.
- 10878 EXCLUDED: Oct 2026 comment shows 4-bit save/load now works; remaining bug tracked in #13439 with PR #23703.
- 16892 EXCLUDED: claimed by AbdulAliMamnun (prototype requested by B-Step62).
- 13643 EXCLUDED: contested (rahuja23 fork, kriscon-db PR #25714, maintainer skeptical).
=> mlflow DEMOTED from shortlist: only 2 workable issues; GFIs get PRs within hours.
## sktime
- 7196 EXCLUDED: nearly all splitters done/claimed (CutoffSplitter claimed Oct 13 2026).
- 7285 OK-but-contested: open, unassigned, stalled PR #7300 (failing tests) + several claims since 2024, none finished. fkiraly said "start fresh or build on #7300, credit sank8-2".
- 11329 OK-but-contested: open, unassigned, no upstream PR. Global recursive reduction 60-400x slower than mlforecast on panel data; fkiraly agreed, wants a benchmark. Dishan18 proposed vectorising get_expected_pred_idx in _fh.py; KrishChadha001 has fork commit. -> pick a DIFFERENT sub-bottleneck (mtype conversion / WindowSummarizer rebuild) and coordinate.
- 8384 OK-but-claimed-unconfirmed: weekly cron CI job testing against nightly numpy/pandas/scikit-learn wheels (SPEC 4). Claims Jun 2025 + Jan 2026, no PR.
- 7237 EXCLUDED: PR #7362 covers it.
- 11495 EXCLUDED: PRs #11504, #11497 open.
## xarray
- 11721 OK (tracking): opened 2026-10-13 by headtr1ck; claim ONE item by commenting. Unclaimed examples: remove .dt.weekofyear/.week; remove quantile(interpolation=...); raise on .chunk(chunks=None); reset_encoding uses UserWarning+wrong version; cftime_range docstring cites unreleased version; TreeNode.lineage warns twice; missing whats-new entries. (argmax msg + to_variable already claimed/done.)
- 10591 OK: open, unassigned, no PRs. Structured-dtype _FillValue with zarr v2 -> TypeError in _check_fill_values (set of unhashable void scalar). headtr1ck (Oct 12 2026) gave fix plan: skip masking for structured dtypes in CFMaskCoder.decode, keep fill value in attrs/encoding, test in xarray/tests/backends/test_zarr.py; asked reporter first, "others welcome".
- 9361 EXCLUDED (likely taken): fork commit by senanurcetin Oct 12 2026 adds the requested io.md note.
- 8239 OK (multi-item): open, unassigned, no linked PRs. repo-review suggestions; maintainers want one small PR per item. Unclaimed: PC170 pygrep-hooks, PC191 ruff --show-fixes, MY103/MY104/MY105/MY106 mypy flags, RF101 flake8-bugbear. (PC901 asked for by Sivaani-Janaswamy May 2026.)
## darts
- 2489 EXCLUDED: discussion moved; batching to be a new issue (dennisbader).
- 1278 OK-but-contested: open, unassigned, no PRs. Historical forecasts on series with NaNs (TimeSeries.has_nan exists now via #3195). Tush2602 asked Oct 15 2026, no maintainer reply.
- 3051 EXCLUDED: robrui draft PR #3130. 2685 EXCLUDED: PRs #3170/#3174.
=> darts DEMOTED: only 1 workable fresh issue.
## numpy (note: NumPy has an AI-agent contribution policy; one claimant withdrew citing it)
- 28028 OK: open, unassigned, no PRs; sustain-2026 project. InessaPawson (member, Oct 4 2026) asked for a short "Debugging from an IDE" subsection in doc/source/dev/development_environment.rst.
- 32272 OK-but-contested: open, unassigned, no PRs, project status "Looking for Contributor". ngoldbaum: docs PR describing current .npz container behaviour is fine. yashb98 and Gambit-Checkmate offered in Aug, no PR.
- 30022 EXCLUDED: muhammed-soysl fork commits.
- 28311 OK (partial): open, unassigned, no PRs. Missing descriptions for ndarray.__abs__ and ndarray.__new__ (unclaimed); __int__/__float__/__complex__ claimed by msmith65 Jul 2026. Docstrings go in numpy/_core/_add_newdocs.py.
## pylint
- 9977 OK: open, unassigned, no PRs. ungrouped-imports/wrong-import-order ignore isort custom sections (known_<section>, sections=...); only first/third-party honoured. Pierre-Sassoulas: Needs PR. Medium.
- 9556 OK: open, unassigned, no PRs. using-constant-test (W0125) false positive on `if e.code:` for caught SystemExit; likely astroid inference of SystemExit.code. Needs PR.
- 10630 OK: open, unassigned, no PRs. Extend W0102 dangerous-default-value to os.getenv()/os.environ defaults; Pierre-Sassoulas: implementer must also fix #4659 (stateful defaults). Enhancement, Needs PR.
- 10737 EXCLUDED: PR #11557 open.
## peft (workflow: maintainer must comment "/peft-triage approved" before a PR; reporters usually fix their own issues)
- 3985 EXCLUDED: reporter has fix ready, waiting for triage approval.
- 3955 EXCLUDED: author intends PR; BenjaminBossan approved.
- 3896 EXCLUDED: LE0-Lin offered; design open.
=> peft DEMOTED (no free issues for outsiders; but excellent maintainer responsiveness).
## transformers (policy: first-time contributors must write PRs themselves - no code agents)
- 27975 OK-but-stale: InstructBLIP not supported by image-to-text pipeline (Jan 2024 analysis: two tokenizers). Volunteer from 2024 vanished; mohame54 asked Sep 2026, no reply.
- 30280 EXCLUDED: maintainer says multiprocessing out of scope.
- 34571 EXCLUDED: zucchini-nlp handling internally; PRs #47702/#48564.
=> transformers DEMOTED: no fresh beginner issues free.
## pytorch
- 74602 OK: open (reopened Jul 2026), unassigned, no PRs. scatter_object_list compares group-local my_rank vs src -> fails on subgroups where local != global rank (gloo). rohan-varma: "seems unexpected". Needs repro on main with 2-process gloo on CPU.
- 89197 OK-but-contested: bool tensors on gloo collectives; fitzyracing1 (Oct 2026) says fixed on nightly via #105354 and offered test-only PR.
- 74809 OK-but-contested: gather of permuted numpy tensor; reported fixed; fitzyracing1 proposes regression tests in test_c10d_gloo.py.
## marimo (free issues are mostly TypeScript/frontend)
- 2405 OK: open, unassigned (previous assignee released Feb 2025), no PRs. "Copy as Altair chart" button for mo.ui.data_explorer; mscolnick: generate snippet in frontend + thorough unit tests. TypeScript.
- 2918 OK: open, unassigned, no PRs (only #2925 CSS ids). Config option to hide/disable download menu items (e.g. via [tool.marimo] in pyproject.toml) -> Python config plumbing + TS menu.
- 3825 OK-but-contested: persist collapsed markdown headings; allin2 awaiting scope confirmation since Jul 2026.
## narwhals (fresh bugs are claimed within days, often by agent-assisted contributors)
- 4054 EXCLUDED: senanurcetin pushed 12 fork commits.
- 4044 EXCLUDED: author has fix ready pending #4043.
- 4014 SKIPPED: maintainer says upstream (DuckDB/Arrow); at most a docs note.
## pytest
- 8208 OK: open, unassigned, no PRs. Docs: add example plugin combining pytest_configure/unconfigure resource management + parametrized fixtures (nicoddemus). 2021 volunteer never followed up.
## netbox (workflow: comment to volunteer -> maintainer assigns -> then PR; unassigned PRs are closed)
- 23248 OK: open, unassigned, no PRs; needs owner, complexity: low (jeremystretch Oct 8 2026). Add a Tags column to global search results table.
- 23412 OK: open, unassigned, no PRs; needs owner; docs. Show API_TOKEN_PEPPERS example as raw string r'...' + warn about backslashes (docs/configuration/).
- 23237 OK: open, unassigned, no PRs; needs owner, severity low. Related Objects of object-type custom fields not clickable -> link to filtered list view.
- 22165 EXCLUDED: mderekasir claimed with patch ready.
## pymc (policy: no formal assignment; opening a PR = claiming)
- 6408 OK-but-unsettled: make test suite fail on unexpected warnings (filterwarnings=error), incremental per-module rollout suggested; approach not agreed; prior offers (2023, Mar 2026) without PR.
- 5791 EXCLUDED: likely done via PR #6514.
=> pymc DEMOTED.
- (pytest) 12235 OK: open, unassigned, no PRs. Maintainers reject base-class change; want OutcomeException docstring/docs note explaining why it inherits BaseException (src/_pytest/outcomes.py). Tiny first PR.
## mitmproxy
- 8012 OK: open, unassigned, no PRs, no maintainer response yet. Delete all flows matching a filter expression (e.g. `~b satisfy`) - could be a core command (view.flows.remove with filter) + UI button.
- 5696 EXCLUDED: open PRs #8212, #8129.
- 8283 EXCLUDED: maintainer Prinzhorn prefers addon approach.
- (pytest) 12876 EXCLUDED: KeepJoyTod proposed docs PR Aug 2026.
- (pytest) 13266 OK-verify-first: Read the Docs LaTeX build failure (sphinxcontrib inkscape converter _StrPath concat, upstream sphinx#13403); no PRs; may be fixed upstream since Mar 2025 -> rebuild docs locally to confirm.
- (jax) 20293 EXCLUDED: "GFI magnet" - PR #21660 + >10 'can I take this' comments. Good example of what NOT to do.
- (networkx) only 3 fresh issues w/o PR, all discussion-type.
- (mitmproxy) 8088 OK: open, unassigned, no PRs. Happy Eyeballs (RFC 8305): pass happy_eyeballs_delay=0.25 to asyncio.open_connection in mitmproxy/proxy/server.py (~L211) so broken IPv6 falls back to IPv4 quickly. No maintainer response yet.
## zulip (workflow: '@zulipbot claim', auto-unassign on inactivity; read contributor guide first)
- 16094 OK-but-contested: i18n on /stats; previous claimant auto-unassigned Aug 2026 but says still working; Chart.js migration pending.
- (zulip) 10888 EXCLUDED: akshatj0630 reproducing (Sep 2026).
=> zulip DEMOTED to honorable mention (all free issues contested / frontend).
- (mitmproxy) 6888 OK: open, unassigned, no PRs; help wanted (mhils). Windows MSI upgrade leaves stale ARP (Add/Remove Programs) entries -> winget re-offers update. Needs Windows VM + installer (InstallBuilder/WiX) knowledge.
