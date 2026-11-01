# TanStack Query v5 — Behavioral Spec for a 100%-Parity GPUI Port (rs-query)

RESEARCH ONLY. All statements below were verified against the source at
`/home/z/my-project/tanstack-query` (shallow clone). File paths are relative to the
repo root (i.e. `packages/query-core/src/...`). Line numbers refer to the current
checkout (a recent v5.x that includes `environmentManager.ts`, `streamedQuery.ts`,
`timeoutManager.ts`, and the new `queryClient.query()`/`infiniteQuery()` APIs).

Corrections to assumptions in the task brief are marked **⚠ CORRECTION**.

---

## 1. RETRY (`packages/query-core/src/retryer.ts`)

The retryer is a single state machine shared by queries and mutations. It owns:
`promise`, `status()` (`'pending' | 'resolved' | 'rejected'`), `cancel`, `continue`,
`cancelRetry`, `continueRetry`, `canStart`, `start`.

### 1.1 Default retry count
- `const retry = config.retry ?? (isServerEnvironment() ? 0 : 3)` — `retryer.ts:251`.
- Server detection (`packages/query-core/src/utils.ts:116`):
  `typeof window === 'undefined' || 'Deno' in globalThis` (static snapshot), routed through
  `packages/query-core/src/environmentManager.ts` → `isServer()`; override with
  `environmentManager.setIsServer(() => bool)` (added for Service Workers etc.).
- So: browser default = **3 retries** (4 total attempts); SSR default = **0**.

### 1.2 `retry` value semantics (retryer.ts:257–260)
- `true` → retry infinitely.
- `number N` → retry while pre-increment `failureCount < N` (0-based at check time ⇒ N retries
  after the first attempt).
- `fn(failureCount, error) => boolean` → custom predicate; `failureCount` is 0-based for the
  *first retry decision*.
- `false`/`0`/`undefined-on-server` → no retry; the first rejection settles the promise.
- A synchronously-thrown `queryFn` error counts: `createRetryer` wraps `config.fn()` in
  try/catch and converts to `Promise.reject` (retryer.ts:236–240).

### 1.3 retryDelay — exact formula (**⚠ CORRECTION: no jitter**)
- Default: `function defaultRetryDelay(failureCount) { return Math.min(1000 * 2 ** failureCount, 30000) }`
  (retryer.ts:87–89). Grep for "jitter" across the whole repo: **zero matches**. There is
  **no jitter** in the default delay.
- The delay is computed with the **pre-increment** failureCount (retryer.ts:253–256 run before
  `failureCount++` at line 268): 1st retry waits `2^0*1000 = 1000ms`, 2nd `2000ms`, 3rd `4000ms`,
  … capped at `30000ms`.
- `retryDelay` may be a constant number (fixed delay every retry) or `fn(failureCount, error)`.
- The sleep goes through `sleep()` → `timeoutManager.setTimeout` (`utils.ts:526-530`), so a
  custom `TimeoutProvider` also controls retry pacing.

### 1.4 failureCount / failureReason bookkeeping
- On each *retryable* failure: `failureCount++` **then** `config.onFail(failureCount, error)`
  (retryer.ts:268–271) ⇒ `onFail` sees a **1-based** count. Query maps this to a `'failed'`
  dispatch: `state.fetchFailureCount = failureCount`, `state.fetchFailureReason = error`
  (`query.ts:939-944`). Observer result exposes these as `failureCount`/`failureReason`
  (`queryObserver.ts:737-738`).
- On the *final* failure (no more retries) the retryer rejects **without** calling `onFail`;
  the query's `'error'` action then does `fetchFailureCount: state.fetchFailureCount + 1` and
  sets `fetchFailureReason` (`query.ts:977-991`). Net effect: `failureCount` equals the total
  number of failed attempts after the final error (e.g. `retry: 3`, all 4 attempts fail ⇒
  `failureCount === 4`).
- Both reset to `0`/`null` on every new fetch (`'fetch'` action → `fetchState()`,
  `query.ts:1027-1046`) and on success (`'success'` action clears them unless `manual`,
  `query.ts:961-976`).

### 1.5 What is retried vs. not
- Any rejection (async or sync-throw) from the fetch fn is retryable, subject to `retry`.
- **Cancellation is never retried**: `cancel()` rejects the promise directly and marks the
  retryer resolved/rejected, so the in-flight attempt's rejection is ignored
  (`isResolved()` guard, retryer.ts:244–248). `CancelledError` has `revert`/`silent` flags
  (retryer.ts:120–128).
- `cancelRetry()` sets `isRetryCancelled = true`: **stops further retries but lets the current
  attempt finish** (result still cached); `continueRetry()` re-enables. Used when the last
  observer unsubscribes without a consumed abort signal (`query.ts:638–648`).
- `undefined` query data is a hard error *outside* the retryer: after `retryer.start()` resolves,
  `query.fetch` throws `Error('<queryHash> data is undefined')` (and logs a console.error in dev)
  (`query.ts:868-878`). It is not retried.
- `initialPromise` (from hydration) is used as attempt #1 only (`failureCount === 0`),
  retryer.ts:232–233.

### 1.6 Retry pause conditions
After the delay, the retry continues only if `canContinue()`:
`focusManager.isFocused() && (networkMode === 'always' || onlineManager.isOnline()) && canRun()`
(retryer.ts:183–186). Otherwise it **pauses** (not skips) and resumes via `continue()`:
- Retries pause when the document is hidden — for **all** network modes, including `'always'`
  (focus is checked unconditionally). Docs cross-check:
  `docs/framework/react/guides/query-retries.md` ("Background Retry Behavior").
- Retries pause when offline for `online` and `offlineFirst` modes (see §2).
- `canRun` for queries is `() => true`; for mutations it is `mutationCache.canRun(mutation)`
  (scope serialization, §6.5).

### 1.7 Mutation retry default
- `retry: this.options.retry ?? 0` — `packages/query-core/src/mutation.ts:327`. **Mutations never
  retry by default** (docs: `docs/framework/react/guides/mutations.md:270`).
- `fetchQuery`/`queryClient.query()` force `retry = false` when unset
  (`queryClient.ts:613-615, 663-665`) — imperative fetches don't retry.

---

## 2. NETWORK MODE (`retryer.ts`, `query.ts`, `queryClient.ts`)

- Values: `export type NetworkMode = 'online' | 'always' | 'offlineFirst'`
  (`packages/query-core/src/types.ts:339`). Default `'online'`.
- `canFetch(networkMode)`: `(networkMode ?? 'online') === 'online' ? onlineManager.isOnline() : true`
  (`retryer.ts:96–100`). So **starting** a fetch requires online only in `online` mode.
- `canStart()` = `canFetch(networkMode) && canRun()` (retryer.ts:188). If it can't start,
  `start()` parks on `pause(canStart)` — it resumes automatically once online (the pause's
  `continueFn` re-evaluates `canStart` on every `continue()`).
- `fetchState(data, options)` (`query.ts:1027-1046`), applied on the `'fetch'` dispatch:
  - `fetchStatus: canFetch(networkMode) ? 'fetching' : 'paused'`
  - `fetchFailureCount: 0`, `fetchFailureReason: null`
  - if `data === undefined`: also `error: null, status: 'pending'` (an offline initial mount
    stays `status: 'pending'` + `fetchStatus: 'paused'` — the docs' spinner gotcha,
    `docs/framework/react/guides/network-mode.md`).

### 2.1 Per-mode semantics (verified against `docs/framework/react/guides/network-mode.md`)
- **`online`** (default): fetch won't fire offline → `fetchStatus: 'paused'`; state (status) is
  untouched. Going offline *during* a fetch/retry pauses the retryer (§1.6). On reconnect the
  paused fetch **continues** (not a refetch). `refetchOnReconnect` defaults to true.
- **`always`**: `canFetch` always true; never `paused`; retries don't pause on offline, but DO
  still pause when the document is hidden (§1.6). `refetchOnReconnect` defaults to **false**
  (`queryClient.ts:1108-1111`: `refetchOnReconnect ??= networkMode !== 'always'`).
- **`offlineFirst`**: first attempt always runs; retries behave like `online` (pause while
  offline). Default mode when a `persister` is set (`queryClient.ts:1116-1118`).

### 2.2 Reconnect flow (exact order)
`onlineManager` fires `online: true` → `QueryClient.mount()`'s subscription
(`queryClient.ts:107-119`):
1. `await queryClient.resumePausedMutations()` — no-op if offline; else
   `mutationCache.resumePausedMutations()` = `Promise.all(paused.map(m => m.continue().catch(noop)))`
   (`mutationCache.ts:359-367`).
2. `queryCache.onOnline()` → for every query: first observer with
   `shouldFetchOnReconnect()` → `refetch({ cancelRefetch: false })`; then
   `retryer.continue()` for paused fetches (`query.ts:597-604`, `queryCache.ts:386-392`).

Same structure for focus: `focusManager` focused → resumePausedMutations → `queryCache.onFocus()`
→ `query.onFocus()` = `shouldFetchOnWindowFocus()` observer refetch (`cancelRefetch: false`) +
`retryer.continue()`.

### 2.3 Paused fetch + unmount
When the last observer unsubscribes while `fetchStatus === 'paused' && status === 'pending'`,
the retryer is **cancelled with `{ revert: true }`** (a paused initial fetch is never cached);
otherwise only `cancelRetry()` (`query.ts:635-651`).

---

## 3. CACHING LIFECYCLE

### 3.1 gcTime & the GC timer (`packages/query-core/src/removable.ts`)
- Default: `5 * 60 * 1000` on the client, `Infinity` on the server
  (`Removable.updateGcTime`, removable.ts:42-48; uses `environmentManager.isServer()`).
- `updateGcTime` keeps the **maximum** seen so far: `Math.max(this.gcTime || 0, newGcTime ?? default)`
  — updating options can only lengthen the GC window, never shorten it.
- `scheduleGc()` clears any pending timer, then if `isValidTimeout(gcTime)` (number, `>= 0`,
  `!== Infinity`, utils.ts:146-148) schedules `optionalRemove()` via `timeoutManager.setTimeout`.
  - `gcTime: Infinity` ⇒ no timer ever (GC disabled).
  - `gcTime: 0` ⇒ timer fires on the next macrotask (removal is not synchronous!).
- Timer **starts**: in the `Query`/`Mutation` constructor; on `addObserver` it's cleared
  (`query.ts:612-621`); re-armed when the **last observer** unsubscribes (`query.ts:650`) and
  after **every fetch settles** (`query.ts:929-931` — `finally { this.scheduleGc() }`).
- `Query.optionalRemove` (`query.ts:374-378`): removes from cache only if
  `observers.length === 0 && state.fetchStatus === 'idle'` (never GC a fetching query).
  `Mutation.optionalRemove` (`mutation.ts:229-237`): if still `pending`, re-schedule GC instead
  of removing.
- Max practical delay ≈ 24.8 days (setTimeout 32-bit); `timeoutManager.setTimeoutProvider`
  exists for coalescing/longer timers (`timeoutManager.ts:19-47`).

### 3.2 Active vs. inactive
- **Active** = at least one observer whose `enabled` resolves `!== false`
  (`Query.isActive`, `query.ts:469-473`, via `resolveQueryValue(observer.options.enabled, query)`).
- **Inactive** = zero observers (regardless of data). "Disabled" = `isDisabled()`:
  with observers → `!isActive()`; without observers → `queryFn === skipToken || !isFetched()`
  (`query.ts:484-490`). `isFetched()` = `dataUpdateCount + errorUpdateCount > 0`.
- `isStatic()` = any observer has `staleTime === 'static'` (`query.ts:506-515`); static queries
  are excluded from `refetchQueries`.

### 3.3 staleTime & isStale
- Default `staleTime: 0` ⇒ data is stale immediately. May be a `fn(query)` (`types.ts:200-201`)
  or the sentinel `'static'` (`types.ts:191`).
- `isStaleByTime(staleTime = 0)` (`query.ts:561-576`), in order:
  1. `data === undefined` → stale (always, even `'static'`);
  2. `staleTime === 'static'` → never stale (even if invalidated);
  3. `state.isInvalidated` → stale;
  4. else `!timeUntilStale(dataUpdatedAt, staleTime)` where
     `timeUntilStale = Math.max(updatedAt + (staleTime || 0) - Date.now(), 0)` (utils.ts:157-159).
- `Query.isStale()` (`query.ts:533-543`): **with observers** it defers to
  `observers.some(o => o.getCurrentResult().isStale)` (each observer's own staleTime/enabled);
  **without observers** it is `data === undefined || isInvalidated`.
- Observer-level `isStale` in results = `enabled !== false && query.isStaleByTime(staleTime)`
  (`queryObserver.ts:968-976`).
- Observers schedule a stale-flip timer: `timeUntilStale(...) + 1` ms (the +1 compensates for
  timers firing 1ms early), only when mounted, enabled, not server, and result not already
  stale (`queryObserver.ts:510-532`).

### 3.4 Fetch dedup (single in-flight promise per query)
`Query.fetch` (`query.ts:703-723`): if `state.fetchStatus !== 'idle'` **and** the retryer
hasn't rejected:
- if `state.data !== undefined && fetchOptions.cancelRefetch` → `cancel({ silent: true })` the
  current fetch and start a new one (the old fetch's promise piggybacks on the new retryer
  promise, `query.ts:891-895`);
- else → `continueRetry()` and **return the existing retryer promise** (dedup).
`cancelRefetch` defaults: `true` for `observer.refetch()`/`refetchQueries()`/`fetchNextPage`
(`queryObserver.ts:476`, `queryClient.ts:539`), `false` for focus/reconnect-triggered refetches
(`query.ts:586,600`).
Also note: if `options` are passed to `fetch()` they are applied via `setOptions` **only when a
new fetch actually starts** (`query.ts:726-728`).
If the query has no `queryFn`, it adopts the options of the first observer that has one
(`query.ts:732-737`); otherwise `ensureQueryFn` returns a fn rejecting
`Missing queryFn: '<queryHash>'` (utils.ts:642-673).

### 3.5 Structural sharing — `replaceEqualDeep` exact algorithm (`utils.ts:389-435`)
**⚠ CORRECTION (v5 latest)**: only *plain* arrays and *plain* objects are structurally shared.

```
replaceEqualDeep(a, b, depth = 0):
1. if a === b: return a                      // reference/identity equal (also covers equal primitives)
2. if depth > 500: return b                  // recursion cap, returns NEW value
3. array  = isPlainArray(a) && isPlainArray(b)
   // isPlainArray: Array.isArray && value.length === Object.keys(value).length (no holes, no extra props)
4. if !array && !(isPlainObject(a) && isPlainObject(b)): return b
   // isPlainObject: toString tag '[object Object]', constructor/prototype checks (utils.ts:476-519)
5. iterate b's items/keys:
   - aItem === bItem → copy aItem, count as equal (objects require hasOwn)
   - either side null or non-object → copy bItem
   - else recurse (depth+1); if result === aItem, count equal
6. return (aSize === bSize && equalItems === aSize) ? a : copy
```

Consequences a port must reproduce:
- **Dates, RegExps, Maps, Sets, class instances are NOT structurally shared**: two distinct
  instances that "mean" the same thing are always replaced by `b` (step 4) — only `a === b`
  identity is reused. Docs agree: "Structural sharing only works with JSON-compatible values"
  (`docs/framework/react/guides/important-defaults.md`, StructuralSharing note).
- If *every* item is equal (and sizes match), the **old object/array identity `a` is returned** —
  this is what makes downstream memoization stable.
- Object keys missing from `b` are dropped; `b`'s key order defines the copy's order.
- `replaceData(prev, data, options)` (`utils.ts:542-565`) wraps it: custom
  `structuralSharing(prev, data)` fn > `false` → raw `data` > default `replaceEqualDeep`. In dev,
  a throw from replaceEqualDeep is logged with "Structural sharing requires data to be JSON
  serializable…" and rethrown (`utils.ts:549-560`).
- Applied at: `Query.setData` (`query.ts:392`), `placeholderData` adoption
  (`queryObserver.ts:674`), and `select` results (`queryObserver.ts:697`).

### 3.6 dataUpdatedAt & state reducers (`query.ts`)
- `successState(data, dataUpdatedAt?)` (`query.ts:1054-1062`):
  `data, dataUpdatedAt ?? Date.now(), error: null, isInvalidated: false, status: 'success'`.
  `'success'` action also does `dataUpdateCount++` and, when not `manual`, resets
  `fetchStatus: 'idle'`, `fetchFailureCount: 0`, `fetchFailureReason: null`
  (`query.ts:961-976`).
- `'error'` action (`query.ts:977-991`): keeps existing `data`, sets `error`,
  `errorUpdateCount++`, `errorUpdatedAt: Date.now()`, `fetchFailureCount + 1`,
  `fetchFailureReason = error`, `fetchStatus: 'idle'`, `status: 'error'`, **`isInvalidated: true`**
  (a background error invalidates the previously cached data ⇒ next mount/focus refetches).
- `setQueryData`: `functionalUpdate(updater, prevData)`; updater returning `undefined` is a
  no-op that does **not** create the query (`queryClient.ts:283-316`); written with
  `{ manual: true }` → `fetchStatus` untouched and `#revertState` = newState (so a later
  `cancel({revert:true})` reverts a manual write too, `query.ts:972-974`).

### 3.7 initialData (`query.ts:1070-1105`)
- `initialData` fn called **once** at state creation; `hasData` ⇒ `status: 'success'`,
  `dataUpdatedAt = initialDataUpdatedAt ?? Date.now()` (so initialData is stale by default with
  `staleTime: 0`); `initialDataUpdatedAt` fn evaluated only when there is data.
- `setOptions` on an existing data-less query adopts new `initialData` via a full
  `successState` setState and refreshes `#initialState` (`query.ts:362-371`).
- `#initialState` is also what `reset()` restores (`resetState` getter, `query.ts:450-462`).

### 3.8 skipToken & disabled queries
- `skipToken = Symbol()` (`utils.ts:628`). `defaultQueryOptions` forces `enabled: false` when
  `queryFn === skipToken` (`queryClient.ts:1120-1122`). Unlike plain `enabled: false`,
  `refetch()` cannot start it (ensureQueryFn rejects; dev logs
  "Attempted to invoke queryFn when set to skipToken…").

---

## 4. INVALIDATION & REFETCH (`packages/query-core/src/queryClient.ts`)

### 4.1 invalidateQueries (`queryClient.ts:492-512`)
1. `queryCache.findAll(filters)` → each `query.invalidate()` — sets `isInvalidated: true` only if
   not already set (`query.ts:680-684`), dispatching (notifying observers). Invalidation alone
   does not fetch.
2. Unless `filters.refetchType === 'none'`: `refetchQueries({ ...filters, type: filters.refetchType ?? filters.type ?? 'active' })`.
   ⇒ **default refetch target is ACTIVE queries only**; invalidated inactive queries simply sit
   stale in the cache and refetch on next mount/focus/reconnect (subject to `refetchOnMount` etc.).
3. Returned promise resolves when the refetches settle.

### 4.2 refetchQueries (`queryClient.ts:533-557`)
- `cancelRefetch` defaults `true`.
- Filter with `findAll(filters)` (default `type: 'all'` in `matchQuery`, `utils.ts:218-236`),
  then **exclude** `query.isDisabled()` and `query.isStatic()` queries.
- If the query is currently `paused`, it resolves immediately for that query (doesn't wait for
  the network).
- Errors swallowed unless `throwOnError` (`RefetchOptions`).

### 4.3 Filter matching (`utils.ts:205-251`)
`matchQuery`: `queryKey` (partial via `partialMatchKey`, or exact via `queryHash` string
equality), `type` ('all' | 'active' | 'inactive'), `stale` (boolean vs `query.isStale()`),
`fetchStatus`, `predicate`. All specified filters must match. `find` defaults `exact: true`
(`queryCache.ts:327`).

### 4.4 cancelQueries (`queryClient.ts:459-472`)
- `cancelOptions` default **`{ revert: true }`**.
- `Promise.all(query.cancel(...)).then(noop).catch(noop)` — never rejects.
- `Query.cancel` (`query.ts:427-431`) rejects the retryer promise with
  `CancelledError{revert, silent}` and calls `onCancel` (`query.ts:844-852`): if `revert` →
  `setState({ ...#revertState, fetchStatus: 'idle' })` (restores the state captured when the
  fetch started — `#revertState` is saved at `query.ts:828`), and `abortController.abort()`.
- In `fetch`'s catch (`query.ts:890-905`):
  - `silent` → return the *replacement* fetch's promise (no error surfaced);
  - `revert` → if there is data, **resolve with the restored `state.data`**; if no data
    (initial fetch cancelled) → rethrow `CancelledError` (state stays reverted, no 'error' dispatch);
  - otherwise (non-revert cancel) → `'error'` dispatch with the `CancelledError` (status becomes
    `error`, `isInvalidated: true`).
- Abort signal consumption: `queryFnContext.signal` is a lazily-defined getter that marks
  `#abortSignalConsumed = true` the first time it is *read* (`query.ts:747-760`). If the queryFn
  never reads `signal`, aborting has no effect on the transport; on last-observer-unmount the
  query is only `cancelRetry()`ed (see §2.3) — this is why "transport doesn't support
  cancellation ⇒ let the fetch finish and cache the result".

### 4.5 resetQueries (`queryClient.ts:420-440`)
- `matched.forEach(query.reset())` — `reset()` = `destroy()` (clears GC timer + **silent
  cancel**) + `setState(#initialState)` (`query.ts:439-462`): back to `initialData`/empty
  pending, `isInvalidated: false`.
- Then `refetchQueries({ type: 'active', predicate: q => matched.has(q) })`.

### 4.6 removeQueries / clear (`queryClient.ts:395-404`, `queryCache.ts:235-260`)
- `removeQueries`: `findAll(filters)` → `queryCache.remove(query)`; `remove` destroys the query
  (clears GC timer + silent cancel) and deletes it from the Map, notifying `'removed'`.
  Guarded by instance identity (`queryInMap === query`).
- `queryCache.clear()` / `queryClient.clear()` remove everything in one `notifyManager.batch`.

---

## 5. INFINITE QUERIES

### 5.1 Wiring
- `InfiniteQueryObserver.setOptions/getOptimisticResult` set `options._type = 'infinite'`
  (`infiniteQueryObserver.ts:108-143`); `queryClient.fetchInfiniteQuery/infiniteQuery` do the same.
- `Query.fetch` swaps in the behavior when `queryType === 'infinite'`:
  `infiniteQueryBehavior(this.options.pages)` **overriding** `options.behavior`
  (`query.ts:819-825`).

### 5.2 `infiniteQueryBehavior.onFetch` full flow (`packages/query-core/src/infiniteQueryBehavior.ts`)
State captured at fetch start: `direction = fetchOptions.meta.fetchMore?.direction`,
`oldPages = state.data?.pages || []`, `oldPageParams = state.data?.pageParams || []`,
`result = { pages: [], pageParams: [] }`.

- **fetchNextPage** (`infiniteQueryObserver.ts:165-174`) → `fetch({ meta: { fetchMore: { direction: 'forward' } } })`
  with `cancelRefetch` default `true`. In the behavior: if `oldPages.length`, compute
  `param = getNextPageParam(options, oldData)` (from **last** page) and `fetchPage(oldData, param, previous=false)`.
- **fetchPreviousPage** → direction `'backward'`; `param = getPreviousPageParam(options, oldData)`
  (from **first** page; requires the option to exist). Pages are **prepended**
  (`addToStart`), dropping the **last** page when `maxPages` exceeded.
- `fetchPage(data, param, previous?)` (`infiniteQueryBehavior.ts:49-88`):
  - if `cancelled` → `Promise.reject(context.signal.reason)`;
  - **if `param == null && data.pages.length` → resolve current data unchanged** (this is how
    `getNextPageParam` returning `undefined`/`null` ⇒ no new page; `hasNextPage` false);
  - context passed to queryFn: `{ client, queryKey, pageParam: param, direction: previous ? 'backward' : 'forward', meta, signal }`;
  - `signal` is consume-aware (`addConsumeAwareSignal`, `utils.ts:714-742`): first read registers
    `abort` listener that sets `cancelled = true` (stops page loops on cancel);
  - result built with `addToEnd`/`addToStart(page, maxPages)` (`utils.ts:597-614`): **only one
    item dropped per insert**, so a pre-existing overflow can persist.
- **Refetch (no direction meta)** (`infiniteQueryBehavior.ts:101-118`):
  `remainingPages = pages ?? oldPages.length`; loop while `currentPage < remainingPages`:
  `param = currentPage === 0 ? (oldPageParams[0] ?? options.initialPageParam) : getNextPageParam(options, result)`;
  `if (currentPage > 0 && param == null) break;` — i.e. a refetch re-fetches **the first N pages
  with their original pageParams**, then follows `getNextPageParam`. If the first cached
  pageParam is `undefined`, `initialPageParam` is used.
- **maxPages trimming direction**: forward fetch drops the **oldest/first** page
  (`addToEnd` → `slice(1)`), backward fetch drops the **newest/last** page (`addToStart` →
  `slice(0, -1)`). `pageParams` are trimmed in lockstep with `pages`.
- **hasNextPage / hasPreviousPage** (`infiniteQueryBehavior.ts:186-207`):
  `hasNextPage = data != null && getNextPageParam(...) != null` (null *or* undefined ⇒ false);
  `hasPreviousPage` additionally requires `getPreviousPageParam` to be provided.
- **Result flags** (`infiniteQueryObserver.ts:207-251`): `isFetchingNextPage` /
  `isFetchNextPageError` / `isFetchingPreviousPage` / `isFetchPreviousPageError` are derived
  from `state.fetchMeta.fetchMore.direction` combined with `isFetching`/`isError`;
  `isRefetching`/`isRefetchError` exclude page-fetches.
- **refetchPage was removed in v5** (docs `docs/framework/react/guides/infinite-queries.md`;
  v5 migration guide) — replaced by the `maxPages` + `pages` option on
  `queryClient.infiniteQuery({ pages: N })`.
- **select** on infinite queries runs on the whole `InfiniteData` object in the observer's
  `createResult` (same code path as normal queries, `queryObserver.ts:685-708`), and is
  memoized per (data reference, select fn identity); the selected result is structurally
  shared against the previous selected result; a `select` throw is captured into the result
  (`status: 'error'`, `errorUpdatedAt: Date.now()`, `isPlaceholderData: false`).

### 5.3 Pending page-fetch tracking
There is no separate "pending pages" queue: a `fetchNextPage` while a fetch is in flight
reuses/cancels via the same `cancelRefetch` semantics as §3.4 (with `cancelRefetch: true` default,
a running refetch is silently cancelled and replaced by the page fetch; `cancelRefetch: false`
returns the in-flight promise and the page is NOT fetched). `fetchMeta` records the direction so
the UI can distinguish `isFetchingNextPage` from `isRefetching`.

---

## 6. MUTATIONS (`packages/query-core/src/mutation.ts`, `mutationCache.ts`, `mutationObserver.ts`)

### 6.1 State & lifecycle
`MutationState`: `{ context, data, error, failureCount, failureReason, isPaused, status,
variables, submittedAt }` (mutation.ts:30-75). Status: `idle → pending → success | error`
(`status: 'idle'` before first `mutate`).

Reducer details (`mutation.ts:464-532`):
- `'pending'` (variables, context?, isPaused): `data: undefined, failureCount: 0,
  failureReason: null, error: null, status: 'pending', variables, submittedAt: Date.now()`.
  ⇒ **`submittedAt` is stamped on every 'pending' dispatch** (including the post-`onMutate`
  re-dispatch, §6.3).
- `'success'`: `data, failureCount: 0, failureReason: null, error: null, status: 'success',
  isPaused: false`.
- `'error'`: `data: undefined, error, failureCount: state.failureCount + 1,
  failureReason: error, isPaused: false, status: 'error'`.
- `'pause'`/`'continue'`: flip `isPaused`.

### 6.2 `execute(variables)` ordering (mutation.ts:301-462)
For a fresh (non-restored) mutation:
1. dispatch `'pending'` (with `isPaused = !retryer.canStart()` — i.e. offline or scope-blocked
   ⇒ pending+paused from the very start);
2. `await mutationCache.config.onMutate(variables, mutation, ctx)` (global, no return value used);
3. `await options.onMutate(variables, ctx)` → `context`;
4. if `context !== state.context` → dispatch `'pending'` again carrying the context;
5. `await retryer.start()` (mutationFn through retries/networkMode/scope queue);
6. success path, each awaited **in this order**:
   `cache.config.onSuccess` → `options.onSuccess` → `cache.config.onSettled` →
   `options.onSettled` → **then** dispatch `'success'` → return data.
7. error path: `cache.config.onError` → `options.onError` → `cache.config.onSettled` →
   `options.onSettled` → dispatch `'error'` → rethrow original error. **Each of the four
   callbacks is individually try/caught** (throwing callbacks become unhandled rejections and
   never mask the original error).
8. `finally`: drop the settled retryer reference and `mutationCache.runNext(this)` (scope queue).

Consequences to port faithfully:
- Observers see `status: 'success'` only **after** `onSuccess`/`onSettled` (mutation options)
  complete — async callbacks delay the state transition.
- The onMutate context is passed as the last argument to onError/onSettled (and onSuccess).
- `mutationFnContext = { client, meta, mutationKey }` (no signal; cancellation of mutations is
  not supported).
- Restored (dehydrated) mutation: `execute` skips steps 1–4, dispatches `'continue'`, and goes
  straight to `retryer.start()`; `Mutation.continue()` (mutation.ts:260-269): has retryer →
  `retryer.continue()`; status still pending (dehydrated in-flight) → `execute(state.variables!)`;
  settled → resolve immediately.

### 6.3 mutate() per-call callbacks (`mutationObserver.ts:217-320`)
`MutationObserver.mutate` builds a **new** Mutation in the cache per call (previous one is
detached, kept in cache until its gcTime), adds the observer, and executes. The per-call
`MutateOptions.onSuccess/onError/onSettled` are invoked by the observer's `#notify` **when the
`'success'`/`'error'` action is dispatched**, only if the observer **has listeners**
(`hasListeners()`), inside a `notifyManager.batch`; `mutateOptions` is snapshotted first so a
callback calling `mutate()` again can't mix options across mutations. Order relative to §6.2:
per-call callbacks run **after** the mutation-level callbacks (they're triggered by the
dispatch that follows them).

### 6.4 Offline pause & auto-resume
- `retryer.canStart()` false (offline in `online` mode) → `start()` parks in `pause(canStart)`;
  `onPause` dispatches `'pause'` ⇒ `state.isPaused = true` and the mutation is queued.
- Reconnect ⇒ `queryClient.resumePausedMutations()` (§2.2) → each paused mutation `continue()`s.
  All paused mutations are continued **in parallel** (`Promise.all`), but scope serialization
  (`canRun`) still gates actual execution.
- Docs: paused mutations survive `dehydrate` (default `shouldDehydrateMutation` = `isPaused`)
  and resume after `hydrate` (docs/framework/react/guides/mutations.md:283-367).

### 6.5 Mutation scopes (`mutationCache.ts:216-251, 167-207, 375-377`)
- `options.scope.id` (string). `add()` pushes into `#scopes: Map<string, Mutation[]>`.
- `canRun(m)`: the **first `pending` mutation in the scope list** is the only one allowed to run
  (`!firstPending || firstPending === m`). Later mutations start paused and sit in the queue.
- `runNext(m)` (called in `execute`'s `finally`): finds the next mutation in the scope with
  `isPaused && !== m` and calls `continue()`.
- `remove()` splices out of the scope list (deleting the map entry when it empties).
- Scope is purely a **serialization** mechanism — no shared gcTime/persistence beyond both
  being dehydrated/restored like any mutation (**⚠ CORRECTION vs. "share gcTime"**: gcTime is
  per-mutation via `Removable`; the docs describe scope only as "run in serial, start in
  isPaused: true" — docs/framework/react/guides/mutations.md:399-407).

### 6.6 useMutationState (`packages/react-query/src/useMutationState.ts:183-206`)
Subscribes to `mutationCache` (`useSyncExternalStore` + `notifyManager.batchCalls`), snapshots
`mutationCache.getAll()` filtered via `matchMutation` (supports `mutationKey` (partial/exact),
`status`, `predicate`), `select`s each `Mutation`. Every `mutate()` call adds a new cache entry
that lives for its gcTime.

---

## 7. HYDRATION / DEHYDRATION (`packages/query-core/src/hydration.ts`)

### 7.1 dehydrate output
`DehydratedState = { mutations: DehydratedMutation[], queries: DehydratedQuery[] }`:

```
DehydratedQuery {
  dehydratedAt: number            // Date.now() at dehydrate time
  state: QueryState               // full state; data passed through serializeData if defined (only when data !== undefined)
  queryKey: QueryKey
  queryHash: string
  promise?: Promise<unknown>      // ONLY when state.status === 'pending'
  meta?: QueryMeta
  queryType?: 'infinite'
}
DehydratedMutation {
  mutationKey?: MutationKey
  state: MutationState            // full state incl. variables & context
  scope?: MutationScope
  meta?: MutationMeta
}
```

- **Defaults**: `shouldDehydrateMutation` = `mutation.state.isPaused` (only paused mutations);
  `shouldDehydrateQuery` = `query.state.status === 'success'` (**only successful queries**;
  failed/errored queries are NOT dehydrated unless you supply a predicate
  (`hydration.ts:209-221`). Per-call options override `client.getDefaultOptions().dehydrate.*`.
- **Error redaction** (**⚠ CORRECTION on message**): pending-query promise rejections are
  replaced by `new Error('redacted')` unless `shouldRedactErrors?.(error) === false`
  (`hydration.ts:139-165`). Default (predicate undefined) = **redact**. Dev builds log
  "A query that was dehydrated as pending ended up rejecting. [hash]: err; The error will be
  redacted in production builds". The literal message is `'redacted'` (lowercase), not
  `'Redacted Error'`.
- The dehydrated promise chain: `query.promise.then(serializeData).catch(redact)` with a
  `.catch(noop)` to avoid unhandled rejections.

### 7.2 hydrate (`hydration.ts:306-436`)
Mutations first: `mutationCache.build(client, { ...hydrate.defaults.mutations, ...dehydrated
mutationOptions }, state)` — mutations come back with their `pending`/paused state and can be
resumed via `continue()` (needs `mutationFn` from `setMutationDefaults` after a page reload).

Queries, per entry:
1. `syncData = promise ? tryResolveSync(promise) : undefined` — reads a **synchronously
   already-resolved** promise (hydration.ts:26-43).
2. `rawData = state.data === undefined ? syncData?.data : state.data`; then `deserializeData`.
3. **Existing query** (`queryCache.get(queryHash)`): hydrate **only if newer** —
   `state.dataUpdatedAt > query.state.dataUpdatedAt || (syncData && dehydratedAt > query.state.dataUpdatedAt)`.
   On write: `setState({ ...serializedState (minus fetchStatus), data })` — the existing query's
   `fetchStatus` is never overwritten; plus, if `state.status === 'pending' && data !== undefined`
   → force `status: 'success'`, `dataUpdatedAt: dehydratedAt`, and `fetchStatus: 'idle'` unless
   the existing query is actively fetching.
4. **New query**: `queryCache.build(client, { hydrate defaults, queryKey, queryHash, meta,
   _type: queryType }, { ...state, data, fetchStatus: 'idle', status: pending&&data ? 'success' : status, dataUpdatedAt: dehydratedAt (when pending&&data) })`
   — `fetchStatus` is reset to `'idle'` so hydrated queries are never stuck "fetching".
5. **Promise streaming**: if `promise && !syncData && !existingQueryIsPending &&
   !existingQueryIsFetching && dehydratedAt > query.state.dataUpdatedAt` →
   `query.fetch(undefined, { initialPromise: Promise.resolve(promise).then(deserializeData) })`
   (RSC-transformed promises are not thenable hence the `Promise.resolve` wrap). The retryer
   uses `initialPromise` as attempt #1 instead of calling `queryFn` (§1.5); if it later needs a
   retry and still has no queryFn, `ensureQueryFn` returns a fn that re-awaits the (rejected)
   initialPromise (utils.ts:660-666).
- Hydrated queries have **no fetchFn**; the first observer with a queryFn lends its options on
  next fetch (`query.ts:732-737`).
- `initialDataUpdatedAt` (query option, §3.7) is unrelated to dehydrate/hydrate but interacts:
  a hydrated `dataUpdatedAt` is just a state field and participates in staleness normally.

---

## 8. FOCUS / ONLINE / NOTIFY / TIMEOUT MANAGERS

### 8.1 focusManager (`packages/query-core/src/focusManager.ts`)
- Singleton `FocusManager extends Subscribable`. Default setup: on first subscriber,
  `window.addEventListener('visibilitychange', listener, false)`; the listener calls
  `onFocus()` with **no argument** ⇒ re-evaluate `isFocused()` = explicit `setFocused` value ??
  `globalThis.document?.visibilityState !== 'hidden'`. Removed when last subscriber leaves
  (`onSubscribe`/`onUnsubscribe` hooks, focusManager.ts:39-50).
- `setFocused(bool)` only notifies when the value **changes**; `setFocused(undefined)` falls
  back to visibility detection.
- Listeners receive the computed boolean: `queryClient.mount` subscribes and, when `focused`,
  resumes paused mutations then `queryCache.onFocus()`.

### 8.2 onlineManager (`packages/query-core/src/onlineManager.ts`)
- Singleton; **initial state `#online = true`** (assumes online when no events available).
- Default setup: `window.addEventListener('online'/'offline')` → `setOnline(true/false)`;
  notifies only on change. Same subscribe/unsubscribe lifecycle as focusManager.

### 8.3 notifyManager (`packages/query-core/src/notifyManager.ts`)
- `batch(cb)`: `transactions++` → run → on outermost exit `flush()`. **Batches nest**; the
  return value passes through.
- `flush()` swaps the queue and schedules it with `scheduleFn` — the **default scheduler is
  `systemSetTimeoutZero` = `setTimeout(cb, 0)`** (a macrotask) —
  **⚠ CORRECTION: not a microtask by default**. Framework adapters (or tests) can install
  `queueMicrotask`/`requestAnimationFrame` via `notifyManager.setScheduler`.
- Purpose: collapse many synchronous state updates (one invalidate call touches N queries ×
  M observers) into **one** scheduled notification pass; `batchCalls(cb)` wraps an arbitrary
  callback so all its invocations within a tick are batched (used by adapters'
  `useSyncExternalStore` subscribe).
- `setNotifyFunction` (e.g. React `act` in tests) and `setBatchNotifyFunction` (adapter
  batching primitive) wrap each callback / the whole flush.

### 8.4 timeoutManager (`packages/query-core/src/timeoutManager.ts`)
- Singleton wrapper over a pluggable `TimeoutProvider` (setTimeout/clearTimeout/setInterval/
  clearInterval). All internal timers — gcTime, retry delays (`sleep`), stale-flip timers,
  refetch intervals — go through it, so a GPUI port should route them through one replaceable
  timer service. Notifies (dev only) if the provider is switched after use; notes the ~24-day
  setTimeout cap and the coalescing rationale.

---

## 9. GC / CACHE-DETAILS

### 9.1 query.remove() vs clear()
- `queryCache.remove(query)` removes a single instance (identity-guarded) after `destroy()`;
  `queryCache.clear()`/`queryClient.clear()` batch-remove everything. `queryClient.removeQueries`
  is filtered. There is also `Query.destroy()` = clear GC timer + `cancel({ silent: true })`.

### 9.2 Cache notify events (`queryCache.ts:41-93`)
`'added'`, `'removed'`, `'updated'` (with the reducer `action`), `'observerAdded'`,
`'observerRemoved'`, `'observerResultsUpdated'`, `'observerOptionsUpdated'` (only when a
defaulted observer's options actually changed, `queryObserver.ts:218-227`). Every notify is
wrapped in `notifyManager.batch`. The persist plugin treats `'added' | 'removed' | 'updated'`
as "cache changed" events (persist.ts:102-112).

### 9.3 Observers tracking
**⚠ CORRECTION**: `Query.observers` is a plain **Array** (`query.ts:300`), not a Map — dedup by
`includes()`, order = subscription order (`onFocus`/`isStale` use `find`/`some` over it, so
"first observer that wants to fetch" wins).
- `addObserver`: dedupe + `clearGcTimeout()` + `'observerAdded'`.
- `removeObserver`: splice; if last: cancel policy (§2.3) + `scheduleGc()`; `'observerRemoved'`.

### 9.4 Options plumbing / "defaultObserverOptions"
- Query options resolution order (`defaultQueryOptions`, `queryClient.ts:1093-1131`):
  `client defaultOptions.queries` → `getQueryDefaults(queryKey)` (from `setQueryDefaults`,
  merged in **registration order**, later overrides earlier, partial key match) → caller
  options → `_defaulted: true`. Then `queryHash` computed via `queryKeyHashFn ?? hashKey`
  (hashKey = `JSON.stringify` with **recursively sorted object keys**, utils.ts:323-334).
  Dependent defaults: `refetchOnReconnect ??= networkMode !== 'always'`;
  `throwOnError ??= !!suspense`; `persister ⇒ networkMode ||= 'offlineFirst'`;
  `queryFn === skipToken ⇒ enabled = false`.
- Each observer `setOptions` also pushes its options onto the query (`currentQuery.setOptions`),
  so the query's own options trail its most recent observer (plus `getQueryDefaults` merged at
  build time, `queryCache.build` → `client.defaultQueryOptions` + `defaultOptions: client.getQueryDefaults(queryKey)`).

### 9.5 refetchInterval / refetchIntervalInBackground (`queryObserver.ts:534-561`)
- `refetchInterval: number | false | fn(query) => number | false`. `0`/`false`/invalid ⇒ no
  timer. Timer only exists while the observer is **mounted**, `enabled !== false`, and not on
  the server (`#shouldScheduleTimer`, queryObserver.ts:502-508).
- Uses `timeoutManager.setInterval`; on each tick it fetches **only if**
  `refetchIntervalInBackground || focusManager.isFocused()` — polling is suppressed in
  background tabs by default.
- The function form is re-evaluated whenever results update (`#computeRefetchInterval` called
  in `#updateTimers` / `setOptions`), so `refetchInterval: q => q.state.error ? 5000 : 30000`
  works (docs/framework/react/guides/polling.md / query-retries.md).

### 9.6 gcTime=Infinity / 0
- `Infinity` → `isValidTimeout` false → no timer; entry lives until `removeQueries`/`clear`.
  (Server default is `Infinity`, which is how SSR avoids GC.)
- `0` → timer of 0 ms; removal happens on the next macrotask **after** the current synchronous
  work (and only if still no observers and not fetching). The 5-min default also means the GC
  timer is re-armed after each fetch — a refetching-but-unobserved query never GCs mid-flight
  (fetchStatus idle check in `optionalRemove`).

---

## 10. OTHER NOTABLE BEHAVIORS

### 10.1 query.meta
`Query.meta` getter → `options.meta` (`query.ts:323-325`); passed to the queryFn context
(`queryFnContext.meta`) and to behaviors; dehydrated with the query (§7.1). Mutation `meta`
analogous (`mutation.ts:187-189`), passed to `mutationFnContext` and per-call observer notify
context. Typed globally via the `Register` interface (`types.ts:330-336`).

### 10.2 signal propagation (opt-in cancellation)
- Plain queries: `queryFnContext.signal` is a lazy getter (created per attempt) that flags
  `#abortSignalConsumed`; the `AbortController` aborts when the retryer is cancelled
  (`query.ts:747-760, 844-852`). If `queryFn` never reads `signal`, cancellation still rejects
  the query promise, but the app's fetch isn't told, and unmount only calls `cancelRetry()`.
- Infinite queries/streamedQuery use `addConsumeAwareSignal` (utils.ts:714-742) to set
  `cancelled = true` when the consumer reads an aborted signal — page loops and streams check
  it between chunks (`infiniteQueryBehavior.ts:36-44,54-56`; `streamedQuery.ts:98-130`).
- `streamedQuery` (`streamedQuery.ts`): 'pending' until the first chunk, then 'success' while
  `fetchStatus: 'fetching'` continues until the stream ends; refetchMode 'reset' resets to
  `query.resetState` + fetching, 'append' appends via `setQueryData`, 'replace' writes once at
  the end.

### 10.3 Logger
**⚠ CORRECTION**: v5 has **no `logger` option** (grep for `logger|Logger` in
`packages/query-core/src` → zero matches). All diagnostics are direct `console.error`/`console.warn`
calls guarded by `process.env.NODE_ENV !== 'production'` (e.g. missing queryFn, `undefined`
data, skipToken misuse, structural-sharing failure, hydration redaction log).

### 10.4 Experimental persister (`packages/query-persist-client-core/src/createPersister.ts`)
- `options.persister(queryFn, ctx, query)` replaces the direct `queryFn` call for normal
  queries (`query.ts:783-789`) and wraps the whole page-fetching `fetchFn` for infinite queries
  (`infiniteQueryBehavior.ts:122-137`). Setting a persister defaults `networkMode` to
  `'offlineFirst'` (`queryClient.ts:1116-1118`).
- `experimental_createQueryPersister({ storage, buster, maxAge = 24h, prefix,
  refetchOnRestore = true, filters })`: per-query storage entries keyed `prefix-queryHash`;
  restore only when `state.data === undefined`; expired/busted/invalid entries are deleted;
  restores `dataUpdatedAt`/`errorUpdatedAt` afterwards and refetches if stale (or always with
  `'always'`); persists after fetch via `notifyManager.schedule`. Also exposes
  `persisterGc`, `restoreQueries` (via `setQueryData` with the original `updatedAt`),
  `removeQueries`.
- Whole-client persistence (`packages/query-persist-client-core/src/persist.ts`):
  `PersistedClient = { timestamp, buster, clientState: DehydratedState }`; restore discards on
  `maxAge` (default 24h) mismatch/buster mismatch/missing timestamp and on any error (then
  rethrows); subscribe saves on every `'added' | 'removed' | 'updated'` cache event from either
  cache.

### 10.5 isRestoring & `_optimisticResults`
- `useBaseQuery` sets `defaultedOptions._optimisticResults = isRestoring ? 'isRestoring' :
  subscribed ? 'optimistic' : undefined` (`packages/react-query/src/useBaseQuery.ts:83-90`).
- In `createResult` (`queryObserver.ts:616-634`): `'optimistic'` merges `fetchState(...)` into
  the computed result when the observer would fetch on mount/option-change — the very first
  render already shows `isFetching` before effects run. `'isRestoring'` forces
  `newState.fetchStatus = 'idle'` and the adapter skips subscribing (`shouldSubscribe = false`)
  so nothing fetches while the persisted cache restores.
- `getOptimisticResult` **builds** the query into the cache (`queryObserver.ts:290-291`) and
  assigns `#currentResult` if it differs — this is the render-time sync mechanism.

### 10.6 Suspense (`packages/react-query/src/suspense.ts`, `useBaseQuery.ts`)
- `throwOnError` defaults to `!!suspense` (`queryClient.ts:1112-1114`); suspense hooks use
  `defaultThrowOnError = (err, query) => query.state.data === undefined`.
- `ensureSuspenseTimers`: when `suspense`, `staleTime` is clamped to **≥ 1000ms** (keeping
  `'static'`) and numeric `gcTime` to ≥ 1000ms — prevents refetch/GC on remount after
  suspending.
- Suspense "promise dedup" is just the query-level fetch dedup (§3.4): concurrent suspending
  components share the in-flight promise; `fetchOptimistic` races the fetch against a
  cache-updated subscription and never rejects (catch clears the error-boundary reset flag).

### 10.7 notifyOnChangeProps / tracked props
- Result identity: `updateResult` early-returns on `shallowEqualObjects(nextResult, prevResult)`
  (so a state dispatch that changes nothing notifies nobody).
- Default notification = **tracked props only**: the adapter wraps the result in a Proxy
  (`trackResult`) recording reads; a change notifies if any tracked prop changed. `notifyOnChangeProps:
  'all'` or an array overrides; `throwOnError` forces notification when `error` changes
  (`queryObserver.ts:786-820`).

### 10.8 Misc invariants
- `hashKey` sorts object keys recursively; two keys differing only in key order map to the same
  queryHash (`utils.ts:323-334`).
- `partialMatchKey` matches array prefixes positionally and object subsets (b ⊆ a)
  (`utils.ts:343-374`); note `['a']` filter matches `['a','b']` but not `['ab']`.
- `matchMutation` returns false when a `mutationKey` filter is given but the mutation has no key
  (`utils.ts:274-285`).
- `queryClient.isFetching()` counts `fetchStatus === 'fetching'` (not `paused`!);
  `isMutating()` counts `status === 'pending'` (`queryClient.ts:151-173`).
- `QueryCache.config` has global `onSuccess/onError/onSettled` callbacks for queries — always
  called (fire-and-forget), distinct from `defaultOptions` (`queryCache.ts:25-39`).
- `QueryClient.mount()`/`unmount()` are ref-counted (`#mountCount`) — nested providers sharing a
  client don't double-subscribe (`queryClient.ts:103-135`).

---

## PORTING CHECKLIST — behavioral invariants to test in rs-query

**Retry**
1. Default retry = 3 in browser-like env, 0 when `isServer`; override detection hook.
2. No jitter: delay = `min(1000 * 2^k, 30000)` with k = 0-based retry index (1000, 2000, 4000…).
3. `retry: true` ⇒ infinite; fn form receives 0-based failureCount and error.
4. failureCount/failureReason: 1-based in state after each retryable failure; +1 more on final
   error; reset at fetch start and on success.
5. `cancelRetry` lets the current attempt finish; `cancel` rejects immediately; neither retries.
6. Retries pause on hidden document (all network modes) and on offline (online/offlineFirst);
   resume via continue; never skipped.
7. Mutations default `retry: 0`; `fetchQuery/query()` default `retry: false`.
8. `undefined` data from queryFn ⇒ hard error, dev-only console.error, not retried.

**Network mode**
9. `canFetch` gate only for `'online'`; `'offlineFirst'` runs attempt 1 offline; `'always'`
   ignores online entirely (but retries still respect focus).
10. Offline initial fetch ⇒ `status: 'pending'`, `fetchStatus: 'paused'`; retrying fetch going
    offline ⇒ `fetchStatus: 'paused'` with existing state preserved.
11. Reconnect order: resume paused mutations (await) → per-query refetch-on-reconnect
    (cancelRefetch:false) → continue paused retryers.
12. `refetchOnReconnect` default = `networkMode !== 'always'`.
13. Last observer unmount while paused+pending ⇒ revert-cancel; else cancelRetry only.
14. `queryClient.resumePausedMutations` no-ops while offline.

**Cache lifecycle**
15. gcTime default 5min (client) / Infinity (server); longest-wins on updates; Infinity ⇒ no
    timer; 0 ⇒ next-macrotask removal; timer re-armed after each fetch and on last unobserve;
    never removes while fetching (queries) or pending (mutations re-schedule).
16. Active = any observer with enabled !== false; disabled/static queries excluded from
    refetchQueries; `staleTime: 'static'` blocks even `refetchOn*: 'always'` and manual
    invalidation refetches.
17. isStale order: no data ⇒ stale; 'static' ⇒ fresh; invalidated ⇒ stale; else time-based
    (`dataUpdatedAt + staleTime <= now`).
18. Error dispatch keeps old data, sets `isInvalidated: true` (background refetch errors
    invalidate).
19. Fetch dedup: in-flight promise reused; `cancelRefetch: true` (default for refetch/
    fetchNextPage) silently cancels only when data exists; focus/reconnect refetches use
    `cancelRefetch: false`.
20. `replaceEqualDeep`: plain arrays/objects only; identity reuse when fully equal; depth cap
    500; Dates/Maps/Sets/RegExp/class instances replaced wholesale; old data reference returned
    when nothing changed (test with `Object.is` on nested nodes).
21. `setQueryData` with updater returning undefined is a no-op that doesn't create the query;
    manual writes set revertState.
22. initialData: fn called once; `dataUpdatedAt = initialDataUpdatedAt ?? now`; stale by
    default.
23. skipToken ⇒ `enabled: false`; `refetch()` cannot start it.

**Invalidation/cancel/reset**
24. invalidateQueries: marks all matched stale; refetches only active (refetchType default
    'active'; 'none' skips); promise awaits refetches.
25. cancelQueries default `revert: true`: state restored to pre-fetch snapshot + fetchStatus
    idle; with data, the awaited fetch resolves with old data; without data it rejects with
    CancelledError; non-revert cancel dispatches an error state.
26. Signal consumption is lazy: cancellation only propagates to the transport if the queryFn
    read `context.signal`.
27. resetQueries: destroy + setState(initialState) then refetch active matched.
28. removeQueries destroys + notifies 'removed'; identity-guarded; clear() batches.

**Infinite**
29. Data shape `{pages, pageParams}`; pageParams recorded per page; refetch re-uses
    `oldPageParams[0] ?? initialPageParam` then follows getNextPageParam up to `pages ?? old count`.
30. `getNextPageParam` → null/undefined ⇒ no next page (fetchNextPage resolves with unchanged
    data); same for previous via getPreviousPageParam (which must also be defined).
31. maxPages: forward trims from the front, backward trims from the back, pages+pageParams in
    lockstep, single-item trim per fetch.
32. fetchNextPage/Previous default `cancelRefetch: true`; direction recorded in fetchMeta and
    surfaced as isFetchingNextPage/isFetchNextPageError etc.; refetching flags exclude page
    fetches.
33. Infinite behavior replaces `options.behavior` when `_type === 'infinite'`.

**Mutations**
34. Callback order success: cache.onMutate → options.onMutate → (2nd 'pending' dispatch with
    context) → mutationFn → cache.onSuccess → options.onSuccess → cache.onSettled →
    options.onSettled → dispatch 'success'; error path rethrows original error, per-callback
    try/catch.
35. Observers transition to success/error only after option callbacks settle; per-call
    mutate() callbacks fire on dispatch and only if the observer has listeners.
36. submittedAt stamped at each 'pending' dispatch (0 when idle); failureCount/failureReason
    mirror retryer.
37. Offline mutations: pending+isPaused at start; queued; parallel continue on reconnect;
    scope.id serializes via first-pending rule and runNext-on-finally.
38. Mutation gc: pending mutations are never removed (GC re-scheduled); dehydrated paused
    mutations resume via continue → execute(last variables).

**Hydration**
39. dehydrate defaults: successful queries only; paused mutations only; `dehydratedAt`
    timestamp; promise attached only for pending queries; errors redacted to `Error('redacted')`
    by default with dev-side logging.
40. hydrate: newer-data guard (`dataUpdatedAt` compare; `dehydratedAt` for sync-resolved
    promises); existing query's fetchStatus never overwritten; fetchStatus reset to idle on new
    queries; pending+data ⇒ success with dataUpdatedAt = dehydratedAt.
41. Streaming: initialPromise reused as attempt #1 (no queryFn call); skip fetch when promise
    resolved synchronously or existing query pending/fetching.
42. serializeData/deserializeData applied symmetrically (data + promise payload).

**Managers/notify**
43. focusManager: visibilitychange-driven; manual setFocused overrides; notifies only on change;
    listeners attached on first subscribe, detached on last unsubscribe.
44. onlineManager: assumes online initially; window online/offline events; change-only notify.
45. notifyManager: nested batch with outermost flush; default scheduler setTimeout(0)
    (macrotask — schedule RS microtask/Frame queue equivalent deliberately); batchCalls helper.
46. All internal timers (gc, retry sleep, stale flip +1ms, refetch interval) flow through one
    replaceable timeout service.
47. Refetch interval: only while mounted+enabled+client; skipped in background unless
    refetchIntervalInBackground; fn form re-evaluated on result updates.
48. Cache events: added/removed/updated/observerAdded/observerRemoved/observerResultsUpdated/
    observerOptionsUpdated, all batched; persist plugin saves on added/removed/updated only.
49. hashKey: recursive key-sort JSON; queryHash identity = dedup identity.
50. useMutationState/useIsRestoring equivalents: cache subscription + filters + select;
    isRestoring forces fetchStatus idle and disables subscription.
51. Suspense parity: throwOnError defaults to suspense; staleTime/gcTime clamped to ≥1000ms;
    suspense dedup = fetch dedup; suspense throwOnError = "no data" only.
52. notifyOnChangeProps default = tracked (read) props; shallowEqualObjects gate on result
    updates; throwOnError forces error-change notification.
