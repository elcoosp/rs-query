#!/usr/bin/env bash
# commit-per-file.sh — one Conventional Commit per changed file (macOS / bash 3.2 safe)
#
#   ./commit-per-file.sh --dry-run     # show the plan, touch nothing
#   ./commit-per-file.sh               # create the commits
#   ./commit-per-file.sh --no-verify   # skip git hooks
#
# Notes:
#   * Requires a clean index (nothing already staged) and no merge/rebase in progress.
#   * Never pushes, never amends, never rewrites history.
#   * Files are committed in dependency order (config -> core -> api -> docs -> ci -> release).

set -euo pipefail
IFS=$'\n\t'

if ! command -v python3 >/dev/null 2>&1; then
  printf 'error: python3 is required (xcode-select --install)\n' >&2
  exit 1
fi

exec python3 - "$@" <<'PYEOF'
import fnmatch
import os
import subprocess
import sys
import textwrap

USAGE = """\
usage: commit-per-file.sh [options]

  -n, --dry-run        print the commit plan without staging or committing
      --no-verify      pass --no-verify to git commit (skip hooks)
      --exclude PAT    skip paths matching glob PAT (repeatable)
      --include-all    clear the default excludes (changed.txt)
  -h, --help           show this help
"""

# ---------------------------------------------------------------- curated data
# (path, subject, body) in intended commit order.
CURATED = [
    ("Cargo.toml",
     "chore(deps): update deps, adopt edition 2024 and pedantic lints",
     """
Prepare the crate for the TanStack parity work and for CI gating.

- Bump dependencies and move the crate to Rust edition 2024.
- Add optional serde / serde_json / chrono dependencies plus the optional gpui-component git dependency used by the devtools feature.
- Add pretty_assertions and a tokio dev-dependency (rt-multi-thread, rt, time, sync, macros, test-util) for the deterministic async suites.
- Declare [lints.rust] missing_docs = "warn" and a pedantic [lints.clippy] group so findings fail the build under RUSTFLAGS=-D warnings.
- Keep default features empty; devtools stays opt-in via dep:chrono and dep:gpui-component.
"""),

    ("src/network.rs",
     "feat(network): add NetworkMode for offline-aware fetching",
     """
Model TanStack's networkMode option.

- Add NetworkMode with the always / online / offline-first variants and use it when deciding whether a query or mutation may start or must pause.
- Default stored entries to NetworkMode::Always and expose the flag to the executor and the mutation cache.
"""),

    ("src/state.rs",
     "feat(state): add mutation status and snapshot state types",
     """
Extend the shared state types for the full mutation lifecycle.

- Add MutationStatus (idle -> pending -> success | error) and the MutationState fields behind it: variables, context, data, error, failure_count, failure_reason, is_paused and submitted_at.
- Add MutationSnapshot so cache entries and observers can expose every part of a mutation, including its optional query key.
- Keep the query state timestamps and update counters used by the new observer result flags in one place.
"""),

    ("src/filters.rs",
     "feat(filters): add refetch filters and mutation matching",
     """
Complete the filter helpers used by the refetch and mutation APIs.

- Add RefetchQueryFilters with filters, cancel_refetch and throw_on_error, plus builder methods, Clone and Debug.
- Add MutationFilters and match_mutation: prefix key match unless exact, active/stale resolution and predicate override.
- Cover defaults, overrides and Debug output with unit tests.
"""),

    ("src/retryer.rs",
     "feat(retryer): add shared retry core for queries and mutations",
     """
Extract the behavioural retry core used by both executors.

- Exponential backoff delay(k) = min(1000 * 2^k, 30_000) ms with no jitter, so the first retry waits exactly one second.
- RetryDecision accepting a bool, a fixed count or a predicate over the error and attempt number.
- Drive the attempt loop with pause (offline / hidden) and cancel support, surfacing failure counts and reasons to the caller.
"""),

    ("src/timeout_manager.rs",
     "feat(timeout): add pluggable sleep provider for deterministic timers",
     """
Make time injectable so the suites no longer depend on wall-clock sleeps.

- Add TimeoutManager with a SleepProvider trait and a TokioSleepProvider implementation for production use.
- Route refetch intervals, GC scheduling and retry delays through the manager so tests can advance time explicitly.
- Re-export the provider types from the crate root.
"""),

    ("src/updater.rs",
     "feat(updater): add functional updater helpers",
     """
Add the functional update helpers shared by the cache setters.

- Add Updater and functional_update so set_query_data style APIs accept either a value or a transformation of the previous value.
- Keep the Arc-based signature required by structural sharing.
"""),

    ("src/sharing.rs",
     "refactor(sharing): rework structural sharing around Arc values",
     """
Fit structural sharing to the new state types and lint gates.

- Take Arc arguments in the deep-replacement helper to resolve the previous lifetime error and preserve identity when data is unchanged.
- Simplify the recursion and keep the sharing tests green under pedantic clippy.
"""),

    ("src/query.rs",
     "feat(query): extend query state and fetch metadata",
     """
Grow the query primitive towards the TanStack state shape.

- Track invalidation, fetch metadata and the update counters consumed by the observer result flags.
- Keep stored defaults (gc_time, network_mode) on the entry and re-arm GC after each fetch instead of while one is in flight.
"""),

    ("src/query_cache.rs",
     "feat(query-cache): expose cache entries and change notifications",
     """
Make the query cache observable for devtools, persistence and hooks.

- Expose the cache and its entries publicly so adapters no longer need internal accessors.
- Emit add / remove / update style notifications and keep monotonic gc_time updates, never collecting while a fetch is pending.
"""),

    ("src/executor.rs",
     "feat(executor): align fetch execution with TanStack semantics",
     """
Tighten the fetch executor behaviour.

- Deduplicate concurrent fetches onto a single executor, honour cancel/revert semantics and keep retry state consistent with the retryer.
- Resolve the previous move errors with downcast_ref plus clone and simplify JoinError handling.
"""),

    ("src/infinite.rs",
     "feat(infinite): complete infinite query fetch and page handling",
     """
Finish the infinite query implementation.

- Fix the compilation and borrow conflicts around page params and support max_pages on client-level infinite fetches.
- Add next/previous page fetch handling with the associated fetching and error flags.
- Adjust trait bounds so structural sharing works for paginated data.
"""),

    ("src/streamed_query.rs",
     "feat(streamed-query): add streamed query support",
     """
Add a streamed query helper for chunked responses.

- Write chunks into the cache as they arrive with reset / append / replace refetch modes.
- Keep the accumulation loop cancel-aware so an aborted fetch cannot publish stale chunks.
"""),

    ("src/mutation_cache.rs",
     "feat(mutation-cache): add mutation cache with scoped serial runs",
     """
Introduce the mutation side of the cache, modelled on TanStack's MutationCache.

- Add MutationCache, MutationCacheEntry, MutationScope and MutationCacheConfig with on_mutate / on_success / on_error / on_settled cache-level callbacks.
- Track the full lifecycle (idle -> pending -> success | error) with variables, context, failure counts and submitted_at timestamps via now_ms.
- Serialize mutations sharing a scope id in submit order (canRun / runNext semantics) and expose subscribe() for cache events.
- Garbage-collect settled entries after gc_time (five minutes by default) and support find_all style filtering.
- Hand-write Debug impls rendering callbacks as "<fn>" so configs and entries stay printable.
- Add unit tests covering sequential execution of two scoped mutations and the Debug output.
"""),

    ("src/mutation.rs",
     "feat(mutation): add scoped mutations, retries and callback ordering",
     """
Rework the mutation executor on top of the new mutation cache.

- Replace inline closure fields with the OnMutateFn / OnSuccessFn / OnErrorFn / OnSettledFn aliases and document each hook against its TanStack counterpart.
- Add MutateOptions (builder, Default, Debug) for per-call on_success / on_error / on_settled callbacks.
- Support mutation_key, MutationScope serialization, retry / retry_delay, meta and throw_on_error.
- Run on_mutate before mutate_fn, keep its rollback context and apply the rollback when the mutation ultimately fails.
- Document and implement the exact side-effect order: mutation callback, then cache config callback, then per-call callback, with on_settled last; afterwards invalidate the declared keys and update the cache entry.
- Honour NetworkMode so mutations pause while offline and can be resumed later.
"""),

    ("src/observer.rs",
     "feat(observer): expand observer result flags and state exports",
     """
Bring QueryObserverResult closer to the TanStack observer result shape.

- Add data_updated_at, error_updated_at, error_update_count, failure_count and failure_reason.
- Add the is_fetched, is_fetched_after_mount, is_loading_error, is_refetch_error, is_placeholder_data, is_paused, is_enabled, is_stale and is_refetching flags.
- Export QueryStateUpdate and QueryStateVariant so downstream crates and the devtools can consume observer notifications.
"""),

    ("src/queries_observer.rs",
     "feat(queries): add QueriesObserver and use_queries hook",
     """
Add the multi-query observer required for useQueries parity.

- Hold a Vec<QueryObserver> behind one entity, support dynamic spec length changes and a top-level subscribed flag.
- Return per-query results and allow combining them through a combine closure, with per-query placeholder data.
"""),

    ("src/client.rs",
     "feat(client): expose mutation cache and expand QueryClient surface",
     """
Complete the QueryClient API and route mutations through the cache.

- Own a MutationCache, execute mutations through it and expose scoped submission plus resume_paused_mutations().
- Honour RefetchQueryFilters (filters, cancel_refetch, throw_on_error) on the invalidation and refetch paths.
- Keep default query options such as gc_time and network_mode on stored entries.
- Grow the suite substantially (mutation success, error and rollback paths, on_mutate failures, invalidation and staleness) and switch format! calls to inline captured arguments for the pedantic lint gate.
"""),

    ("src/persistence.rs",
     "feat(persistence): persist and restore queries and mutations",
     """
Add persistence parity with the TanStack persister packages.

- Persist dehydrated queries and paused mutations through a pluggable persister hook with configurable serialize / deserialize.
- Restore entries on startup and re-arm GC and staleness from the stored timestamps.
"""),

    ("src/broadcast.rs",
     "feat(broadcast): sync query and mutation caches across clients",
     """
Add cross-client cache synchronization.

- Broadcast cache events and apply remote updates without echoing them back to the sender.
- Keep the event taxonomy aligned with the query and mutation cache notifications.
"""),

    ("src/hydration.rs",
     "feat(hydration): add dehydrate and hydrate parity helpers",
     """
Add the dehydration / hydration boundary helpers.

- Dehydrate successful queries and paused mutations by default and redact errors in the serialized payload.
- Hydrate into an existing client, restoring state counters and timestamps so observers behave as if freshly fetched.
"""),

    ("src/hooks.rs",
     "feat(hooks): add GPUI hooks for queries and mutations",
     """
Expose the client through GPUI-friendly hooks.

- Add query and mutation hooks wired to the observers so entities re-render on state changes.
- Keep subscription lifetimes tied to the owning entity and make the hooks compile under the pedantic lint gate.
"""),

    ("src/devtools.rs",
     "refactor(devtools): tidy gpui imports and satisfy pedantic clippy",
     """
Keep the devtools panel building under the new lint gates.

- Trim and regroup the gpui imports (App, Context, Entity, EventEmitter, FontWeight, KeyBinding, Render, RenderOnce, SharedString, Styled, ...) after the prelude wildcard was dropped.
- Adjust the panel to the updated QueryClient and QueryStateVariant surface without changing behaviour.
"""),

    ("src/lib.rs",
     "feat(lib): re-export new public API and document every item",
     """
Surface the new modules and keep the public API fully documented.

- Re-export the mutation cache types and MutationScope, TimeoutManager with SleepProvider / TokioSleepProvider, and Updater with functional_update.
- Re-export the observer state types and the new filter helpers.
- Add doc comments for every public item so missing_docs stays clean under the new lint configuration.
"""),

    ("docs/research/01-query-core.md",
     "docs(research): inventory TanStack query-core public API",
     """
Add the file-by-file audit of @tanstack/query-core v5.105.0.

- Inventory query, queryCache, queryObserver, mutation, mutationCache, retryer, focusManager, onlineManager and the infinite / streamed helpers.
- Record default options, state shapes and reducer transitions with source line references.
- Flag what rs-query already covers, what is partial and what is missing so the port plan can be tracked against real evidence.
"""),

    ("docs/research/02-framework-and-persistence.md",
     "docs(research): audit framework hooks and persistence packages",
     """
Extend the source audit to the framework and persistence layers.

- Cover the react-query hooks (useQuery, useQueries, useMutation, useMutationState, useIsFetching) and the observer wiring behind them.
- Document query-persist-client-core, the sync-storage and async-storage persisters and the broadcast-client experiment.
- Note the dehydrate defaults: successful queries only, paused mutations only, errors redacted.
"""),

    ("docs/research/03-behavioral-spec.md",
     "docs(research): specify exact runtime behavior and constants",
     """
Write down the exact behaviour the port must reproduce.

- Retryer: delay(k) = min(1000 * 2^k, 30_000) ms with no jitter, retry as bool | count | predicate, pause and cancel handling.
- Mutation.execute side-effect order, per-callback error isolation and the pending / success / error reducer payloads, including submittedAt stamping on every pending dispatch.
- Filter matching semantics (prefix unless exact, active / stale, predicate override) and GC rules (monotonic gc_time, never while pending).
- Quick-reference table of literal constants: five minute gcTime, retry 3 for queries and 0 for mutations, 30s backoff cap, suspense clamps and dehydrate defaults.
"""),

    ("docs/port-plan.md",
     "docs(plan): add TanStack Query 100% parity port plan",
     """
Turn the source audit into a tracked implementation plan.

- Target full TanStack Query parity audited against query-core v5.105.0, linking the three research documents as raw evidence.
- Split the work into client / executor semantics, mutation cache with scope serialization, persistence, broadcast and hydration parity, GPUI hooks, QueriesObserver, streamedQuery and devtools upkeep.
- List the remaining gaps as checklists: observer option forms, infinite query flags and bidirectional max_pages trimming, structural sharing depth cap, cache event taxonomy.
- Define the test matrix mapping behavioural invariants to suites plus the CI, changelog and README deliverables.
"""),

    (".github/workflows/ci.yml",
     "ci: add fmt, clippy, test and coverage workflow",
     """
Add continuous integration for pushes to main and every pull request.

- Jobs for rustfmt check, pedantic clippy, tests on default and all features, and cargo llvm-cov coverage.
- Set CARGO_TERM_COLOR=always, CARGO_INCREMENTAL=0 and RUSTFLAGS=-D warnings so findings fail the build, with permissions limited to contents: read.
- Upload lcov.info to Codecov and keep it as a coverage-report artifact with if-no-files-found: error.
"""),

    (".github/workflows/release.yml",
     "ci(release): generate changelog and GitHub release on v* tags",
     """
Automate releases from v*.*.* tags.

- Check out full history so git-cliff can walk every tag and verify the test suite before publishing.
- Regenerate CHANGELOG.md from Conventional Commits using cliff.toml and extract the latest entry into release-notes.md.
- Commit the changelog back to main as github-actions[bot] with a chore(release) message, tolerating a no-op when nothing changed.
- Publish the release with softprops/action-gh-release@v2 using the generated notes; permissions are limited to contents: write.
"""),

    ("cliff.toml",
     "chore(release): configure git-cliff conventional changelog",
     """
Configure git-cliff for Conventional Commit changelogs.

- Keep a Changelog style header, SemVer note and a generated-by footer, with trimming enabled.
- Group commits into Features, Bug Fixes, Performance, Documentation, Tests, Refactoring, Styling, Reverted, Security and Miscellaneous Tasks, skipping chore(release) noise.
- Render scope and breaking-change markers, sort commits oldest first and match tags with v[0-9].* so CHANGELOG.md and release notes share one template.
"""),

    ("CHANGELOG.md",
     "docs(changelog): add generated changelog for 0.1.0",
     """
Add the changelog generated by git-cliff.

- Group the initial history into Features, Bug Fixes, Documentation, Miscellaneous Tasks and Tests using the cliff.toml template.
- Record the QueryClient and executor work, the mutation cache with scope serialization, persistence, broadcast and hydration parity, GPUI hooks and devtools upkeep, plus the deterministic suites at 98.46% line coverage.
"""),

    ("README.md",
     "docs(readme): rewrite docs for the expanded feature surface",
     """
Rewrite the README around the current feature set and workflow.

- Replace the old TanStack comparison table with documentation of the query client, mutation cache, scopes, persistence, broadcast sync, hydration, GPUI hooks and devtools.
- Show the mutation example using mutation_key, MutationScope and an on_mutate rollback closure.
- Document the verification commands (cargo test --all-features, clippy with -D warnings, cargo llvm-cov, cargo doc --no-deps) with the current test and coverage numbers.
- Describe the CI and release workflows, git-cliff versioning, the contribution focus areas and the MIT license.
"""),
]

MESSAGES = {}
ORDER = {}
for _i, (_p, _s, _b) in enumerate(CURATED):
    MESSAGES[_p] = (_s, _b)
    ORDER[_p] = _i

UNMERGED = {"DD", "AU", "UD", "UA", "DU", "AA", "UU"}


def die(msg):
    sys.stderr.write("error: " + msg + "\n")
    sys.exit(1)


def git_out(*args):
    p = subprocess.run(("git",) + args, stdout=subprocess.PIPE,
                       stderr=subprocess.PIPE, universal_newlines=True)
    if p.returncode != 0:
        die("git " + " ".join(args) + " failed:\n" + p.stderr.strip())
    return p.stdout


def git_code(*args):
    p = subprocess.run(("git",) + args, stdout=subprocess.PIPE,
                       stderr=subprocess.PIPE, universal_newlines=True)
    return p.returncode


def wrap_body(body):
    out = []
    for raw in body.strip("\n").split("\n"):
        line = raw.rstrip()
        if not line:
            out.append("")
        elif line.startswith("- "):
            out.append(textwrap.fill(line, width=72, subsequent_indent="  ",
                                     break_long_words=False,
                                     break_on_hyphens=False))
        else:
            out.append(textwrap.fill(line, width=72, break_long_words=False,
                                     break_on_hyphens=False))
    return "\n".join(out).strip("\n")


def fallback(paths, status):
    path = paths[-1]
    base = os.path.basename(path)
    stem = os.path.splitext(base)[0]
    parts = path.split("/")
    if path.startswith(".github/workflows/") or path.startswith(".github/"):
        kind, scope = "ci", stem
    elif path.startswith("tests/") or stem.endswith("_test") or stem == "tests":
        kind, scope = "test", (parts[1] if len(parts) > 2 else stem)
    elif path.startswith("docs/") or path.lower().endswith(".md"):
        kind, scope = "docs", stem.lower()
    elif path.startswith("src/"):
        kind, scope = "feat", stem
    elif base in ("Cargo.toml", "Cargo.lock"):
        kind, scope = "chore", "deps"
    else:
        kind, scope = "chore", (parts[0] if len(parts) > 1 else stem)
    scope = scope.replace("_", "-").lower()
    if status == "added":
        verb, subject = "add", "add " + base
    elif status == "deleted":
        verb, subject = "remove", "remove " + base
    elif status == "renamed":
        verb, subject = "rename", "rename " + os.path.basename(paths[0]) + " to " + base
    else:
        verb, subject = "update", "update " + base
    head = kind + "(" + scope + "): " + subject
    if len(head) > 72:
        head = kind + ": " + subject
    body = (verb.capitalize() + " `" + path + "` as part of the mutation cache "
            "and TanStack parity work.\n\n"
            "- File status: " + status + ".\n"
            "- This path was not covered by the curated message table; refine "
            "the wording with `git commit --amend` if needed.")
    return head, body


def sort_key(item):
    paths, status = item
    path = paths[-1]
    if path in ORDER:
        return (0, ORDER[path], path)
    if path.startswith("src/"):
        rank = 1
    elif path.startswith("tests/"):
        rank = 2
    elif path.startswith("docs/"):
        rank = 3
    elif path.startswith(".github/"):
        rank = 4
    else:
        rank = 5
    return (rank, 0, path)


def main():
    dry_run = False
    no_verify = False
    excludes = ["changed.txt"]

    argv = sys.argv[1:]
    i = 0
    while i < len(argv):
        a = argv[i]
        if a in ("-h", "--help"):
            sys.stdout.write(USAGE)
            return 0
        if a in ("-n", "--dry-run"):
            dry_run = True
        elif a == "--no-verify":
            no_verify = True
        elif a == "--include-all":
            excludes = []
        elif a == "--exclude":
            i += 1
            if i >= len(argv):
                die("--exclude requires a pattern")
            excludes.append(argv[i])
        else:
            die("unknown option: " + a + "\n\n" + USAGE)
        i += 1

    if git_code("rev-parse", "--is-inside-work-tree") != 0:
        die("not inside a git work tree")
    root = git_out("rev-parse", "--show-toplevel").strip()
    os.chdir(root)
    gitdir = git_out("rev-parse", "--git-dir").strip()
    for marker in ("MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD",
                   "rebase-merge", "rebase-apply"):
        if os.path.exists(os.path.join(gitdir, marker)):
            die("a merge/rebase/cherry-pick is in progress; finish it first")

    if git_out("diff", "--cached", "--name-only").strip():
        die("the index is not empty; commit or unstage your staged changes "
            "first (git reset) so each commit stays file-scoped")

    raw = git_out("status", "--porcelain=v1", "-z", "--untracked-files=all")
    records = raw.split("\0")
    items = []
    skipped = []
    idx = 0
    while idx < len(records):
        rec = records[idx]
        idx += 1
        if not rec:
            continue
        xy, path = rec[:2], rec[3:]
        origin = None
        if "R" in xy or "C" in xy:
            if idx < len(records):
                origin = records[idx]
                idx += 1
        if xy in UNMERGED or "U" in xy:
            skipped.append((path, "unmerged"))
            continue
        if xy == "!!":
            continue
        if any(fnmatch.fnmatch(path, pat) or
               fnmatch.fnmatch(os.path.basename(path), pat) for pat in excludes):
            skipped.append((path, "excluded"))
            continue
        if "?" in xy:
            status = "added"
        elif "D" in xy:
            status = "deleted"
        elif "R" in xy:
            status = "renamed"
        else:
            status = "modified"
        paths = [path] if origin is None else [origin, path]
        items.append((paths, status))

    if skipped:
        for path, why in skipped:
            sys.stdout.write("skip (" + why + "): " + path + "\n")
    if not items:
        sys.stdout.write("nothing to commit\n")
        return 0

    items.sort(key=sort_key)
    total = len(items)
    sys.stdout.write(("DRY RUN: " if dry_run else "") +
                     str(total) + " file(s) -> " + str(total) +
                     " commit(s) in " + root + "\n\n")

    done = 0
    for n, (paths, status) in enumerate(items, start=1):
        key = paths[-1]
        if key in MESSAGES:
            subject, body = MESSAGES[key]
        else:
            subject, body = fallback(paths, status)
        if len(subject) > 72:
            sys.stderr.write("warning: subject longer than 72 chars: " +
                             subject + "\n")
        message = subject + "\n\n" + wrap_body(body) + "\n"
        prefix = "[" + str(n) + "/" + str(total) + "] "

        if dry_run:
            sys.stdout.write(prefix + key + "\n")
            sys.stdout.write(textwrap.indent(message, "    ") + "\n")
            continue

        add = ("git", "add", "-A", "--") + tuple(paths)
        p = subprocess.run(add, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                           universal_newlines=True)
        if p.returncode != 0:
            die("git add failed for " + key + ":\n" + p.stderr.strip())
        if git_code("diff", "--cached", "--quiet", "--", *paths) == 0:
            sys.stdout.write(prefix + "no staged change, skipped: " + key + "\n")
            continue
        cmd = ["git", "commit", "-q", "-F", "-"]
        if no_verify:
            cmd.append("--no-verify")
        p = subprocess.run(cmd, input=message, stdout=subprocess.PIPE,
                           stderr=subprocess.PIPE, universal_newlines=True)
        if p.returncode != 0:
            die("git commit failed for " + key + ":\n" +
                (p.stderr.strip() or p.stdout.strip()) +
                "\n(the file is staged; fix the issue and rerun the script)")
        done += 1
        sys.stdout.write(prefix + subject + "\n")

    if dry_run:
        sys.stdout.write("dry run complete, nothing was staged or committed\n")
    else:
        sys.stdout.write("\ncreated " + str(done) + " commit(s); nothing was pushed\n")
        sys.stdout.write("review with: git log --oneline -n " + str(max(done, 1)) + "\n")
    return 0


sys.exit(main())
PYEOF
