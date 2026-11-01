# 02 — TanStack Query: framework-level packages & persistence — exhaustive inventory

Research for the **rs-query** port (TanStack Query → Rust/GPUI). RESEARCH ONLY — no code changes to `rs-query`.

Sources (shallow clone at `/home/z/my-project/tanstack-query`, current `main`):

- `packages/react-query/src/*` (all 25 non-test files read)
- `packages/query-persist-client-core/src/{persist,createPersister,retryStrategies}.ts`
- `packages/query-sync-storage-persister/src/index.ts` (+utils)
- `packages/query-async-storage-persister/src/{index,asyncThrottle}.ts`
- `packages/query-broadcast-client-experimental/src/index.ts`
- `packages/react-query-persist-client/src/PersistQueryClientProvider.tsx`
- `packages/query-core/src/{index,types,hydration,mutation,mutationObserver,query,queryClient,utils}.ts`

> Note: the package is spelled `query-broadcast-client-experimental` in the repo (the prompt said
> `query-broadcast-client-experiment`; there is no `-experiment` package).

---

## 1. `react-query` export surface (`src/index.ts`)

Re-exports **everything** from `@tanstack/query-core`, then adds:

| Export | Kind |
|---|---|
| `useQueries` (+ types `QueriesResults`, `QueriesOptions`) | hook |
| `useQuery` | hook |
| `useSuspenseQuery` | hook |
| `useSuspenseInfiniteQuery` | hook |
| `useSuspenseQueries` (+ `SuspenseQueriesResults`, `SuspenseQueriesOptions`) | hook |
| `usePrefetchQuery` | hook (returns `void`) |
| `usePrefetchInfiniteQuery` | hook (returns `void`) |
| `queryOptions` (+ `DefinedInitialDataOptions`, `UndefinedInitialDataOptions`, `UnusedSkipTokenOptions`) | identity helper (typed) |
| `infiniteQueryOptions` (+ `DefinedInitialDataInfiniteOptions`, `UndefinedInitialDataInfiniteOptions`, `UnusedSkipTokenInfiniteOptions`) | identity helper (typed) |
| `QueryClientContext`, `QueryClientProvider`, `useQueryClient` (+ `QueryClientProviderProps`) | provider |
| `QueryErrorResetBoundary`, `useQueryErrorResetBoundary` (+ `QueryErrorClearResetFunction`, `QueryErrorIsResetFunction`, `QueryErrorResetBoundaryFunction`, `QueryErrorResetFunction`, `QueryErrorResetBoundaryProps`) | error boundary |
| `HydrationBoundary` (+ `HydrationBoundaryProps`) | SSR hydration |
| `useIsFetching` | hook |
| `useIsMutating`, `useMutationState` | hooks |
| `useMutation` | hook |
| `mutationOptions` | identity helper (typed) |
| `useInfiniteQuery` | hook |
| `useIsRestoring`, `IsRestoringProvider` | context |

`UseQueryOptions` = `UseBaseQueryOptions` minus `suspense` (suspense-ness is decided by *which hook*
you call, not an option). `UseBaseQueryOptions` = `QueryObserverOptions` (core) + `subscribed?: boolean`.

---

## 2. `useQuery` / `useBaseQuery` — full option surface

`useBaseQuery(options, Observer, queryClient?)` is shared by `useQuery`, `useInfiniteQuery`,
`useSuspenseQuery`, `useSuspenseInfiniteQuery`. Pipeline, in order:

1. `useIsRestoring()` — if restoring, observers are built with `_optimisticResults: 'isRestoring'`
   (results are NOT optimistic-fetching; queries don't subscribe).
2. `useQueryErrorResetBoundary()` (context; default global boundary).
3. `useQueryClient(queryClient)` — explicit arg wins, else context, else **throws**
   `'No QueryClient set, use QueryClientProvider to set one'`.
4. `client.defaultQueryOptions(options)` — merges `defaultOptions.queries` →
   `getQueryDefaults(queryKey)` → caller options; computes `queryHash` via `queryKeyHashFn`/`hashKey`;
   dependent defaults: `refetchOnReconnect ?? networkMode !== 'always'`,
   `throwOnError ?? !!suspense`, `networkMode ?? (persister ? 'offlineFirst' : …)`,
   `queryFn === skipToken ⇒ enabled = false`.
5. dev-mode guard: throws if `options` is not a plain object (v5 object-only API); warns if no `queryFn`.
6. `subscribed = options.subscribed !== false`; sets `_optimisticResults = isRestoring ?
   'isRestoring' : subscribed ? 'optimistic' : undefined`.
7. `ensureSuspenseTimers(defaultedOptions)` (suspense only, see §5).
8. `ensurePreventErrorBoundaryRetry(...)`: if `suspense || throwOnError` and the boundary has not
   been reset ⇒ force `retryOnMount: false`.
9. `useClearResetErrorBoundary` effect: `errorResetBoundary.clearReset()` on mount.
10. One `Observer` per hook instance (`React.useState(() => new Observer(client, defaultedOptions))`).
11. `observer.getOptimisticResult(defaultedOptions)` before subscribing.
12. `useSyncExternalStore` subscribes via `notifyManager.batchCalls(onStoreChange)` (only if
    `!isRestoring && subscribed`), then `observer.updateResult()` to catch missed updates.
13. Effect: `observer.setOptions(defaultedOptions)` on every render with new defaulted options.
14. If `shouldSuspend(defaultedOptions, result)` ⇒ **throws the promise** `fetchOptimistic(...)`
    (which never rejects; on failure it calls `errorResetBoundary.clearReset()`).
15. If `getHasError({ result, errorResetBoundary, throwOnError, query, suspense })` ⇒ throws
    `result.error` to the error boundary. `getHasError` = `result.isError && !boundary.isReset() &&
    !result.isFetching && query && ((suspense && result.data === undefined) ||
    shouldThrowError(throwOnError, [result.error, query]))`.
16. Returns `observer.trackResult(result)` when no explicit `notifyOnChangeProps` (property-access
    tracking limits re-renders), else raw result.

### 2.1 Options (`UseQueryOptions` — every option, with defaults)

Inherited from core `QueryOptions` + `QueryObserverOptions` (exact names & defaults):

| Option | Type | Default | Behavior |
|---|---|---|---|
| `queryKey` | `QueryKey` (required) | — | hashed (stable JSON-ish `hashKey`; `undefined`/`null`/functions special-cased) → `queryHash`; key change ⇒ new query entry, refetch if enabled |
| `queryFn` | `(ctx) => Promise<T> \| SkipToken` | required unless default queryFn | ctx: `{ queryKey, pageParam?, signal, meta, client }`; `skipToken` ⇒ disabled (see §9); missing ⇒ rejects `Missing queryFn: '<hash>'` |
| `enabled` | `boolean \| (query) => boolean` | `true` | gates automatic refetch on mount/key-change/interval; `refetch()` still works manually |
| `staleTime` | `number \| 'static' \| fn(query)` | `0` | ms before data is stale; `'static'` never stale; fn re-evaluated per check |
| `gcTime` | `number` | `5 * 60 * 1000` (`Infinity` on server) | cache GC after last observer unsubscribes; `Infinity` disables; max ≈24d (`setTimeout` 32-bit) |
| `retry` | `boolean \| number \| (failureCount, error) => boolean` | `3` client / `0` server | retryer attempts |
| `retryDelay` | `(attempt, error) => number` | exponential backoff `min(1000 * 2^attempt + jitter, 30_000)` | |
| `networkMode` | `'online' \| 'always' \| 'offlineFirst'` | `'online'` | offline queueing behavior; `'offlineFirst'` forced when `persister` set |
| `refetchInterval` | `number \| false \| fn(query) => number \| false` | `false` | polling; fn re-evaluated each tick |
| `refetchIntervalInBackground` | `boolean` | `false` | keep polling when window hidden |
| `refetchOnWindowFocus` | `boolean \| 'always' \| fn` | `true` | `'always'` ignores staleTime |
| `refetchOnReconnect` | `boolean \| 'always' \| fn` | `true` unless `networkMode === 'always'` | |
| `refetchOnMount` | `boolean \| 'always' \| fn` | `true` | if stale on mount |
| `retryOnMount` | `boolean \| fn` | `true` | retry an errored query on mount (set `false` by boundary logic, §2 step 8) |
| `notifyOnChangeProps` | `keyof result[] \| 'all' \| fn` | `undefined` = track accessed props | re-render filter |
| `throwOnError` | `boolean \| (error, query) => boolean` | `false` (`!!suspense` after defaulting) | throw to error boundary |
| `suspense` | `boolean` | `false` | **not exposed on `useQuery`** (react-query strips it); used by suspense hooks |
| `select` | `(data: TQueryData) => TData` | — | view transform only; runs only when data changed or fn identity changed; cache untouched |
| `placeholderData` | `TQueryData \| (prev, prevQuery) => TQueryData?` | — | shown while pending; `keepPreviousData` = `(prev) => prev`; does NOT persist to cache; result flagged `isPlaceholderData` |
| `initialData` | `TData \| () => TData` | — | seeds cache; stale by default (unless `staleTime`); **is persisted** |
| `initialDataUpdatedAt` | `number \| () => number \| undefined` | — | freshness anchor for `initialData` |
| `meta` | `Record<string, unknown>` | — | payload on query, visible to `queryFn` ctx & cache events |
| `maxPages` | `number` | `Infinity` | infinite queries only: cap stored pages (trims from the opposite end) |
| `structuralSharing` | `boolean \| (old, new) => unknown` | `true` | `replaceEqualDeep` between results |
| `queryKeyHashFn` | `(key) => string` | `hashKey` | custom hash |
| `persister` | `(queryFn, ctx, query) => Promise<T>` | — | per-query persistence hook (see §10.3) |
| `subscribed` *(react-only)* | `boolean` | `true` | `false` = never subscribe (poll manually via `getCurrentResult`) |
| `_optimisticResults` *(internal)* | `'optimistic' \| 'isRestoring'` | — | set by adapter, not user |

### 2.2 Result (`QueryObserverResult`) — every field

Discriminated union by `status` × `fetchStatus`; base fields:

- `data: TData | undefined` — last successful data (after `select`)
- `dataUpdatedAt: number` — ts of last success
- `error: TError | null`
- `errorUpdatedAt: number`, `errorUpdateCount: number` (sum of all errors)
- `failureCount: number` (retries of current fetch; reset on success), `failureReason: TError | null`
- `status: 'pending' | 'error' | 'success'`
- `fetchStatus: 'fetching' | 'paused' | 'idle'` (`paused` = wanted to fetch but offline/queued)
- Derived booleans:
  - `isPending` (status pending), `isSuccess`, `isError`
  - `isLoading` = `isFetching && isPending` (first fetch in flight; **use this, not isPending, for disabled queries**)
  - `isFetching` (queryFn executing), `isRefetching` = `isFetching && !isPending`
  - `isPaused` (fetchStatus paused)
  - `isFetched` (has ever fetched), `isFetchedAfterMount` (fetched after this observer mounted)
  - `isLoadingError` (failed with no data), `isRefetchError` (failed while refetching, data kept)
  - `isPlaceholderData` (showing `placeholderData`)
  - `isEnabled` (observer enabled)
  - `isStale` (invalidated or older than staleTime)
  - `isInitialLoading` — **deprecated** alias of `isLoading`
- `refetch: (options?: RefetchOptions) => Promise<QueryObserverResult>` with
  `RefetchOptions { throwOnError?: boolean = false; cancelRefetch?: boolean = true }`
  (`cancelRefetch: false` = no-op if a request is already running).
- **No `promise` field on the result** — the suspense promise is *thrown* (§5). (In-flight promises live
  on `query.promise` for SSR streaming / dehydration.)

State-specific narrowing: `QueryObserverPendingResult` (`data: undefined`, `status: 'pending'`),
`LoadingResult` (+`isLoading: true`), `LoadingErrorResult` (`error: TError`), `RefetchErrorResult`
(`data: TData`, `error: TError`), `SuccessResult` (`data: TData`), `PlaceholderResult`
(`data`, `isPlaceholderData: true`). `DefinedQueryObserverResult` = success | refetch-error.

---

## 3. `useInfiniteQuery`

`useBaseQuery(options, InfiniteQueryObserver)`. Options = §2.1 **plus**:

| Option | Default | Behavior |
|---|---|---|
| `initialPageParam` (required) | — | pageParam for first page; only used while `pages` is empty; refetches start from the *first page's own param* |
| `getNextPageParam` (required) | — | `(lastPage, allPages, lastPageParam, allPageParams) => TPageParam \| undefined \| null`; also drives `hasNextPage` |
| `getPreviousPageParam` | — | same shape backwards; drives `hasPreviousPage` |
| `maxPages` | `Infinity` | cap on stored pages (with `InfiniteData` trimming) |
| `pages` *(only on `queryClient.infiniteQuery`/prefetch)* | — | `{ pages: number, getNextPageParam }` to fetch N pages in one call |

Data shape: `InfiniteData<T, P> = { pages: T[]; pageParams: P[] }`.

Extra result fields (`InfiniteQueryObserverBaseResult` ⊇ base result):

- `fetchNextPage(options?: { cancelRefetch?: boolean = true; throwOnError?: boolean = false })` → Promise of result
- `fetchPreviousPage(options?: same)` 
- `hasNextPage`, `hasPreviousPage` (false when the *-PageParam fn returns `undefined`/`null`)
- `isFetchingNextPage`, `isFetchingPreviousPage`
- `isFetchNextPageError`, `isFetchPreviousPageError` (newer additions — do not miss these)

Direction is stored on `query.state.fetchMeta` (`fetchNextPage`/`fetchPreviousPage`/refetch).
Behavior notes: imperative `fetchNextPage` (default `cancelRefetch: true`) cancels/replaces a running
fetch — gate with `hasNextPage && !isFetching`; `refetch()` refetches all pages from page 1 (up to
`maxPages`).

---

## 4. `useMutation`

```
useMutation(options, queryClient?) => { ...MutationState, isIdle, isPending, isSuccess, isError,
  mutate(variables, { onSuccess?, onError?, onSettled? }?), mutateAsync, reset() }
```

Implementation: one `MutationObserver` per hook; effect `observer.setOptions(options)`;
subscribes via `notifyManager.batchCalls`; `mutate` = `observer.mutate(...).catch(noop)`
(fire-and-forget), `mutateAsync = result.mutate` (the raw promise-returning fn).
If `shouldThrowError(observer.options.throwOnError, [result.error])` ⇒ throws `result.error` (error boundary).

### 4.1 Options (`MutationObserverOptions`; `UseMutationOptions` = same minus `_defaulted`)

| Option | Default | Behavior |
|---|---|---|
| `mutationFn` | required unless default via `setMutationDefaults` | `(variables, ctx{client, mutationKey, meta}) => Promise<TData>` |
| `mutationKey` | — | matches `useMutationState`/`isMutating` filters & `setMutationDefaults` |
| `onMutate` | — | `(variables, ctx) => TOnMutateResult \| Promise<...>`; awaited before mutationFn; return value = `context` (rollback data) |
| `onSuccess` | — | `(data, variables, onMutateResult, ctx)`; awaited before `onSettled` |
| `onError` | — | `(error, variables, onMutateResult?, ctx)` |
| `onSettled` | — | `(data?, error?, variables, onMutateResult?, ctx)` |
| `retry` | `0` (mutations do NOT retry by default) | retryer options |
| `retryDelay` | exponential backoff, 30s cap | |
| `networkMode` | `'online'` | offline ⇒ state `isPaused: true`, resumes via `resumePausedMutations()` |
| `gcTime` | `5 * 60 * 1000` | settled mutation evicted from cache after this |
| `scope` | — | `{ id: string }`: mutations sharing an id run **serially** in submit order |
| `meta` | — | cache payload |
| `throwOnError` | `false` | boolean or `(error) => boolean`; thrown after all callbacks |

### 4.2 Side-effect order (exact, from `Mutation.execute`)

1. `dispatch({type:'pending', variables, isPaused})` → `submittedAt = Date.now()`, `failureCount 0`,
   `error null`, `data undefined`; (if state was already `pending` — restored from dehydration —
   `onMutate` is skipped and a `continue` action is dispatched instead)
2. `mutationCache.config.onMutate(variables, mutation, ctx)` (cache-level callback, awaited)
3. `options.onMutate(variables, ctx)` (awaited) → context stored via second `pending` dispatch
4. `mutationFn(variables, ctx)` through retryer (`retry`/`retryDelay`/`networkMode`/scope `canRun`)
   — `onFail` ⇒ `failed` action (failureCount/failureReason), `onPause`/`onContinue` ⇒ `isPaused`
5. **Success:** `cache.config.onSuccess` → `options.onSuccess` → `cache.config.onSettled` →
   `options.onSettled` (each awaited) → `dispatch({type:'success', data})` → promise resolves `data`
6. **Error:** `cache.config.onError` → `options.onError` → `cache.config.onSettled(undefined, error)`
   → `options.onSettled` (each individually try/caught) → `dispatch({type:'error', error})`
   (failureCount+1) → promise **rejects** the original error
7. finally: retryer cleared, `mutationCache.runNext(mutation)` (scope queue)

### 4.3 Per-call callbacks (`mutate(variables, options)`)

Stored as `#mutateOptions` on the observer; fired from `onMutationUpdate` **after** the action
(`success`/`error`) settles — i.e. strictly after the hook-level callbacks — with a *snapshot* of the
options (guard against a newer `mutate` overwriting them). They run **only if the observer still has
listeners** (component mounted); unmounting before settle drops them. Order on success:
`mutateOptions.onSuccess` then `mutateOptions.onSettled`; on error: `onError` then `onSettled`.

### 4.4 Result fields

`MutationState` (also what `useMutationState` returns): `context`, `data`, `error`, `failureCount`,
`failureReason`, `isPaused`, `status`, `variables`, `submittedAt`. Observer result adds
`isIdle`/`isPending`/`isSuccess`/`isError` (from status: `'idle' | 'pending' | 'success' | 'error'`),
`mutate`, `reset` (detach + back to idle; does NOT cancel in-flight).

---

## 5. Suspense family (`useSuspenseQuery`, `useSuspenseInfiniteQuery`, `useSuspenseQueries`)

All force on every query: `{ enabled: true, suspense: true, throwOnError: defaultThrowOnError,
placeholderData: undefined }` and reject `skipToken` as `queryFn` (dev-mode console error).

- `defaultThrowOnError = (_error, query) => query.state.data === undefined` — i.e. **throw only if
  there is no data to show**; a failed background refetch keeps rendering cached data.
- `shouldSuspend(defaultedOptions, result) = options.suspense && result.isPending`.
- `fetchOptimistic(defaultedOptions, observer, boundary)` = `observer.fetchOptimistic(options)`
  (fetch without touching the observer's own result) `.catch(() => boundary.clearReset())` — the
  thrown promise **never rejects**; errors are surfaced by the error-boundary check instead.
- `ensureSuspenseTimers`: when `suspense`, clamp `staleTime` (incl. fn form) and numeric `gcTime` to
  **≥ 1000 ms** (`MIN_SUSPENSE_TIME_MS`; `'static'` kept) so remounts after suspending don't refetch
  or GC the query.
- `useSuspenseQuery` returns `UseSuspenseQueryResult` = defined result **minus `isPlaceholderData`**;
  `useSuspenseInfiniteQuery` likewise (`status: 'success' | 'error'` only).
- Caveats documented: multiple suspense hooks in one component suspend **serially** (waterfall) — use
  `useSuspenseQueries`; cancellation does not work while suspended; component only remounts after all
  queries resolve (stale data may refetch on remount).
- `useSuspenseQueries(options)` = `useQueries` mapped over the same forced fields; **no top-level
  `subscribed`**; supports `combine`.

`useQueries` suspense behavior: if **any** result should suspend, it throws
`Promise.all(suspensePromises)` where each suspending query gets a **fresh throwaway
`new QueryObserver(client, opts)`** + `fetchOptimistic`. Then the first result whose `getHasError`
holds throws its error.

## 5.1 Prefetch helpers

- `usePrefetchQuery(options, queryClient?)` → `void`.
  `if (!client.getQueryState(options.queryKey)) { void client.query(options).catch(noop) }` —
  fires during render; **skipped if the query has any cached state at all** (including leftover
  pending/error state), so repeat renders are cheap. Options = `queryClient.query` options
  (`QueryExecuteOptions`: queryKey required, `staleTime` decides fresh-return, `select` transforms
  only the resolved value) minus `skipToken` queryFn.
- `usePrefetchInfiniteQuery(options)` → `void`; same guard; calls `client.infiniteQuery(options)`
  (requires `initialPageParam`, `getNextPageParam`; optional `{ pages, getNextPageParam }` for
  multi-page prefetch).

---

## 6. `useQueries`

```
useQueries({ queries: [...], combine?, subscribed? }, queryClient?)
```

- `queries`: array of per-query options = `UseQueryOptions` **minus `placeholderData`'s previous-query
  form and minus per-query `subscribed`**; `placeholderData` receives `previousData = undefined`
  always (`QueriesPlaceholderDataFunction`).
- `combine?: (results: QueriesResults<T>) => TCombinedResult` — the ONLY way to get a single object;
  result is structurally shared for referential stability (`replaceEqualDeep` inside
  `QueriesObserver.getOptimisticResult`). `combine` re-runs only when its identity changes or any
  result changed (inline closures run every render — memoize).
- `subscribed?: boolean = true` (top-level only).
- Dynamic arrays supported (`ids.map(...)`); duplicate keys in one array share data (dedupe advised).
- One `QueriesObserver` per hook; `observer.setQueries(defaultedQueries, options)` on change.
- TS layer: tuple-wise inference up to depth 20 (`MAXIMUM_DEPTH`), then homogeneous fallback — port can
  ignore the type gymnastics but keep the *runtime* shape.
- Known limitation documented: inline `select` can't infer from sibling `queryFn` (TS limitation).

---

## 7. Utility hooks

### 7.1 `useIsFetching(filters?, queryClient?)` → `number`
- `filters: Omit<QueryFilters, 'fetchStatus'>` — i.e. `{ type?: 'all'|'active'|'inactive',
  exact?, predicate?, queryKey?, stale? }` (no fetchStatus!).
- Snapshot = `client.isFetching(filters)` (count of queries with `fetchStatus: 'fetching'` after
  filter matching), re-computed on `queryCache.subscribe(...)` (batched by `notifyManager`).

### 7.2 `useIsMutating(filters?, queryClient?)` → `number`
- Implemented as `useMutationState({ ...filters, status: 'pending' }).length`.
- `filters: MutationFilters = { exact?, predicate?, mutationKey?, status? }`.

### 7.3 `useMutationState({ filters?, select? }?, queryClient?)` → `TResult[]`
- `mutationCache.findAll(filters)` mapped through `select ?? (m => m.state)`.
- Snapshot factory per render with caching: recomputes only when the flat list of
  `[mutation, mutation.state]` pairs changed (Object.is on each); passes the new list through
  `replaceEqualDeep(previousResult, …)` so identical selections stay referentially stable.
- Subscribes to `mutationCache.subscribe` (batched).
- `select` receives the full `Mutation` (so you can read `state.variables`, `state.data`,
  `state.submittedAt`, `options.mutationKey`, …). Latest successful invocation = last element of a
  `status: 'success'` filtered list.

### 7.4 `useQueryClient(queryClient?)`
Explicit arg wins; else context; else **throws** `'No QueryClient set, use QueryClientProvider to set one'`.

### 7.5 `QueryClientProvider` / `QueryClientContext`
- `QueryClientContext = createContext<QueryClient | undefined>(undefined)`.
- Props: `{ client: QueryClient; children? }`.
- `useEffect`: `client.mount()` on mount, `client.unmount()` on unmount/client change. `mount()`
  subscribes to focusManager + onlineManager (visibility/online ⇒ refetch stale, resume paused
  mutations); `unmount()` clears them.
- Passing a *new* client re-mounts and re-provides.

### 7.6 `IsRestoringProvider` / `useIsRestoring`
- `createContext(false)`; `useIsRestoring()` reads it; set by `PersistQueryClientProvider`.
- While `true`, hooks build observers with `_optimisticResults: 'isRestoring'` and **do not
  subscribe** (avoids refetch races with the pending restore).

### 7.7 `QueryErrorResetBoundary` / `useQueryErrorResetBoundary`
- Value: `{ reset(), isReset(), clearReset() }` (closure over a `isReset: boolean`; `reset()` sets
  true, `clearReset()` sets false). Context defaults to a module-level value, so hooks always have one.
- `<QueryErrorResetBoundary>{children | fn(value)}</QueryErrorResetBoundary>`; consumers wire an
  `ErrorBoundary`'s `onReset` to `reset`. See §2 steps 8/14/15 for how it gates `retryOnMount` and
  error throwing.

---

## 8. Hydration (React side) — `HydrationBoundary`

Props: `{ state?: DehydratedState | null | undefined; options?: Omit<HydrateOptions,'defaultOptions'>
& { defaultOptions?: Omit<HydrateOptions['defaultOptions'], 'mutations'> }; children?; queryClient? }`
— **`mutations` cannot be defaulted through the boundary** (only `deserializeData` + `queries`).
Children are **always rendered** unconditionally.

Flow (all in `React.useMemo`, i.e. during render — needed for SSR + transitions):

1. Split `state.queries` into `newQueries` (hash not in cache → `hydrate(client, {queries: newQueries},
   options)` **during render**; `hydrate` is idempotent) and `existingQueries` (hash exists AND
   hydration is newer: `dehydrated.state.dataUpdatedAt > existing.state.dataUpdatedAt` OR
   (`dehydrated.promise && existing not pending && not fetching && dehydrated.dehydratedAt >
   existing.state.dataUpdatedAt`)).
2. `existingQueries` are queued and hydrated in a `useEffect` after commit (so aborted React
   transitions don't clobber current-page data).
3. `optionsRef` keeps latest options without re-triggering the memo.

Core `hydrate(client, dehydratedState, options?)` semantics (framework-agnostic, `hydration.ts`):

- Restores `mutations` first: `mutationCache.build(client, {…defaults, …dehydratedMutationOptions}, state)`.
- Queries: `deserializeData` applied; sync-try `promise.then` (`tryResolveSync`) for already-resolved
  streaming promises; existing query updated only if newer (`state.dataUpdatedAt > …` or sync-data
  newer via `dehydratedAt`), **`fetchStatus` omitted** from updates so it isn't stuck; new queries
  built with `fetchStatus: 'idle'`; pending-at-dehydrate + has-data ⇒ hydrated as `success` with
  `dataUpdatedAt: dehydratedAt`; unresolved `promise` re-attached via `query.fetch({initialPromise})`
  (no re-fetch of queryFn).
- `dehydrate(client, options?)` ⇒ `{ mutations, queries }`; per query:
  `{ dehydratedAt: Date.now(), state: {...state, data: serializeData?.(data) ?? data}, queryKey,
  queryHash, promise? (only when status pending), meta?, queryType? ('infinite') }`.
  Defaults: `shouldDehydrateMutation = m => m.state.isPaused` (only paused mutations),
  `shouldDehydrateQuery = q => q.state.status === 'success'` (only successful queries);
  `shouldRedactErrors` — errors of dehydrated pending promises replaced by `Error('redacted')`
  unless the predicate returns `false`; `serializeData` transform. All overridable per-call and via
  `client.defaultOptions.dehydrate` / `.hydrate`.
- `DehydrateOptions`: `{ serializeData?, shouldDehydrateMutation?, shouldDehydrateQuery?,
  shouldRedactErrors? }`. `HydrateOptions`: `{ defaultOptions?: { deserializeData?, queries?,
  mutations? } }`.

---

## 9. `skipToken`

`export const skipToken = Symbol()`; `type SkipToken = typeof skipToken`.

- Pass as `queryFn` to **conditionally disable** a query with full type inference (equivalent of
  `enabled: false`).
- `defaultQueryOptions`: `queryFn === skipToken ⇒ enabled = false`.
- `ensureQueryFn`: attempting to fetch with skipToken ⇒ rejects `Missing queryFn: '<hash>'`
  (plus a dev console.error "Attempted to invoke queryFn when set to skipToken…").
- **`refetch()` cannot trigger a skipToken query** — use `enabled: false` if manual refetch is needed.
- Not allowed on suspense hooks (they need a queryFn) or on prefetch helpers.

---

## 10. `query-persist-client-core` — EXACT API (`src/index.ts` = persist + retryStrategies + createPersister)

### 10.1 Core types

```ts
interface Persister {                       // the whole-cache storage interface
  persistClient: (persistClient: PersistedClient) => Promisable<void>
  restoreClient: () => Promisable<PersistedClient | undefined>
  removeClient: () => Promisable<void>
}
interface PersistedClient {
  timestamp: number            // Date.now() at save; compared against maxAge
  buster: string               // cache-busting string; mismatch ⇒ discard
  clientState: DehydratedState // { queries, mutations }
}
type Promisable<T> = T | PromiseLike<T>
```

Options (interfaces `PersistQueryClientRootOptions` → `…RestoreOptions` → `…SaveOptions` →
`PersistQueryClientOptions`):

| Option | Default | Used by |
|---|---|---|
| `queryClient` | required | all |
| `persister` | required | all |
| `buster` | `''` | restore (compare) + save (stamp) |
| `maxAge` | `24 * 60 * 60 * 1000` (24h) | restore |
| `hydrateOptions` | — (`HydrateOptions`) | restore |
| `dehydrateOptions` | — (`DehydrateOptions`) | save |

### 10.2 Functions

**`persistQueryClientRestore({queryClient, persister, maxAge = 24h, buster = '', hydrateOptions})`**
1. `persistedClient = await persister.restoreClient()`
2. if present:
   - has `timestamp`? → `expired = Date.now() - timestamp > maxAge`;
     `busted = persistedClient.buster !== buster`;
     - expired || busted ⇒ `return persister.removeClient()`
     - else ⇒ `hydrate(queryClient, persistedClient.clientState, hydrateOptions)`
   - no timestamp ⇒ `return persister.removeClient()`
3. on any throw: dev log + warn *“As a precaution, the persisted cache will be discarded.”* ⇒
   `await persister.removeClient()` ⇒ **rethrow**

**`persistQueryClientSave({queryClient, persister, buster = '', dehydrateOptions})`**
- `persister.persistClient({ buster, timestamp: Date.now(), clientState: dehydrate(queryClient, dehydrateOptions) })`

**`persistQueryClientSubscribe(props)` → unsubscribe()**
- `save = () => persistQueryClientSave(props).catch(dev console.error/warn)`
- subscribes **both** caches; only reacts to cache events `type ∈ {'added','removed','updated'}`
  (ignores `observerAdded`, `observerRemoved`, `observerResultsUpdated`, `observerOptionsUpdated`).
  Note: **every cache event saves immediately — throttling lives in the persister, not here.**

**`persistQueryClient(props) → [unsubscribe(), restorePromise]`**
- restore first; on completion (and if not already unsubscribed) start `persistQueryClientSubscribe`.
- `unsubscribe()` flips a flag so a pending restore never starts the subscription.

### 10.3 `experimental_createQueryPersister(options)` — per-query ("fine-grained") persister

⚠️ Distinct mechanism from the whole-cache persister (§10.2): it is passed as the **`persister` option
of a query** (or `defaultOptions.queries.persister`) and wraps `queryFn`; there is **no cache
subscription and no single blob** — each query is its own storage entry.

`StoragePersisterOptions<TStorageValue = string>`:

| Option | Default | Behavior |
|---|---|---|
| `storage` | required (`undefined`/`null` = no-op for SSR) | `AsyncStorage`: `{ getItem(key) → MaybePromise<TStorageValue \| null \| undefined>, setItem(key, value), removeItem(key), entries?() → [key, value][] }` (`entries` needed for GC/restoreQueries/removeQueries) |
| `serialize` | `JSON.stringify` | `(PersistedQuery) => MaybePromise<TStorageValue>` |
| `deserialize` | `JSON.parse` | reverse |
| `buster` | `''` | stored per entry; mismatch ⇒ discard |
| `maxAge` | `24 * 60 * 60 * 1000` | checked against **`state.dataUpdatedAt`** (not save time!) |
| `prefix` | `'tanstack-query'` (`PERSISTER_KEY_PREFIX`) | storage key = `` `${prefix}-${queryHash}` `` |
| `refetchOnRestore` | `true` | `false` never refetch; `'always'` always refetch after restore; `true` only if `query.isStale()` |
| `filters` | — | `QueryFilters` — only matching queries are persisted/restored (`matchQuery`) |

Stored shape `PersistedQuery = { buster, queryHash, queryKey, state: QueryState }`.
Validation `isValidPersistedQuery`: object, string `queryHash`, array `queryKey`, object `state`,
finite `state.dataUpdatedAt`. `isExpiredOrBusted`: no `dataUpdatedAt` ⇒ expired; else
`Date.now() - state.dataUpdatedAt > maxAge || buster mismatch`.

Returns an object with **seven** members:

- `persisterFn(queryFn, ctx, query)` — the query-level `persister`:
  restore only when `query.state.data === undefined` && filter matches; schedule (via
  `notifyManager.schedule`) a post-restore task that fixes `dataUpdatedAt`/`errorUpdatedAt` on the
  live query (`query.setState`) then refetches per `refetchOnRestore`/staleness; else run `queryFn`
  and schedule `persistQuery(query)` after success.
- `persistQuery(query)` — write one entry.
- `persistQueryByKey(queryKey, queryClient)` — find in cache + persist (warns if not found).
- `retrieveQuery(queryHash, afterRestoreMacroTask?)` — read/validate/expire one entry; corrupted ⇒
  `removeItem`; returns `state.data` or `undefined`.
- `persisterGc()` — iterate `storage.entries()`, drop invalid/expired/busted entries (throws in dev
  if `entries` missing).
- `restoreQueries(queryClient, filters? = {})` — bulk restore into cache via
  `setQueryData(key, data, { updatedAt: state.dataUpdatedAt })`; filter by `queryKey` + `exact`.
- `removeQueries(filters? = {})` — bulk delete from storage; same filtering.

Side-effect of using a `persister` in options: `defaultQueryOptions` forces `networkMode: 'offlineFirst'`.

### 10.4 `removeOldestQuery` (retryStrategies.ts)

`type PersistRetryer = ({ persistedClient, error, errorCount }) => PersistedClient | undefined` —
called by storage persisters when `setItem`/`serialize` throws (e.g. quota). `removeOldestQuery`:
copies `{ mutations, queries }` (note: mutations are preserved), sorts queries by
`state.dataUpdatedAt` ascending, removes the oldest **query**, returns the shrunken client; returns
`undefined` when no queries remain (give up).

---

## 11. `query-sync-storage-persister`

`createSyncStoragePersister(options): Persister` — **deprecated in favor of the async persister**.

| Option | Default | Notes |
|---|---|---|
| `storage` | required; `undefined`/`null` ⇒ all methods `noop` | sync `{ getItem(key): string \| null, setItem(key, string), removeItem(key) }` |
| `key` | `'REACT_QUERY_OFFLINE_CACHE'` | single storage key |
| `throttleTime` | `1000` ms | throttle saving |
| `serialize` | `JSON.stringify` | `(PersistedClient) => string` |
| `deserialize` | `JSON.parse` | `(string) => PersistedClient` |
| `retry` | — | `PersistRetryer` (usually `removeOldestQuery`) |

- `persistClient` = **leading-edge throttle** (own impl): first call schedules `func` after `wait`
  (via `timeoutManager.setTimeout` — mockable timers); later calls only overwrite `params` (latest
  wins). Inside the run: `trySave` loop — while `error && client`: `errorCount++`,
  `client = retry?.({persistedClient, error, errorCount})`, retry save.
- `restoreClient`: `getItem(key)` → `deserialize` (sync, returns `PersistedClient | undefined`).
- `removeClient`: `removeItem(key)`.

## 12. `query-async-storage-persister`

`createAsyncStoragePersister(options): Persister` — the recommended one.

Same option surface as §11, except `storage: AsyncStorage<string>` (`getItem/setItem/removeItem` may
return promises) and `retry: AsyncPersistRetryer` (`=> Promisable<PersistedClient | undefined>`).
Defaults identical (`key: 'REACT_QUERY_OFFLINE_CACHE'`, `throttleTime: 1000`, `JSON.stringify`/`parse`).

- `persistClient` = `asyncThrottle(save, { interval: throttleTime })`:
  never concurrent (`isExecuting` loop), runs with the **latest args**, waits until
  `nextExecutionTime = lastRun + interval`; `onError` hook (default noop).
  Same `trySave`/retry loop as sync but awaited.
- `restoreClient` async; `removeClient` async; no storage ⇒ `{ persistClient: noop,
  restoreClient: () => Promise.resolve(undefined), removeClient: noop }`.

## 13. `query-broadcast-client-experimental` — `broadcastQueryClient(...)`

```ts
broadcastQueryClient({
  queryClient,                       // required
  broadcastChannel = 'tanstack-query',
  options?,                          // forwarded to the `broadcast-channel` lib ({webWorkerSupport: false} default)
  onBroadcastError?,                 // (error, {type, queryHash, queryKey}) => void | Promise<void>
}) => () => void                     // unsubscribe + channel.close()
```

What it does exactly:

- Uses the `broadcast-channel` npm package (cross-tab: BroadcastChannel/localStorage/IndexedDB
  transports). Channel name defaults to `'tanstack-query'`.
- Subscribes to `queryCache` and **broadcasts** (via `safePost`, which catches post failures and
  routes them to `onBroadcastError`, or dev `console.warn` — e.g. non-structured-cloneable data):
  - `'updated'` **only when `event.action.type === 'success'`** (fresh data, not error/failure events);
  - `'removed'` **only when the query still has `observers.length > 0`**;
  - `'added'` always (query first observed).
- Message shape: `{ type, queryHash, queryKey, state? }` (`state` for updated/added).
- Applies remote messages inside a **`transaction` guard** (`tx(cb)` sets `transaction = true` in
  try/finally — the finally guarantees reset even if applying throws): while applying, this tab's own
  resulting cache events are ignored so remote updates are never echoed back (this is the
  "local vs remote" suppression — it is a boolean re-entrancy flag, not an origin field).
  - `'updated'`: query exists ⇒ `query.setState(state)`; missing ⇒ `queryCache.build(client, {queryKey, queryHash}, state)`.
  - `'removed'`: query exists ⇒ `queryCache.remove(query)`.
  - `'added'`: query exists ⇒ `setState(state)` **only if `query.state.data === undefined`**; missing ⇒ build.
- Returns unsubscribe = cache unsubscribe + `channel.close()`.

## 14. `react-query-persist-client` — `PersistQueryClientProvider`

Props = `QueryClientProviderProps` (`client`, `children`) **plus**:

| Prop | Type | Behavior |
|---|---|---|
| `persistOptions` | `Omit<PersistQueryClientOptions, 'queryClient'>` | forwarded to restore/subscribe |
| `onSuccess` | `() => unknown \| Promise<unknown>` | after restore resolves — **even if nothing was restored** (expired/empty) |
| `onError` | `() => unknown \| Promise<unknown>` | if restore or `onSuccess` throws |

Flow: `isRestoring` state starts `true`; one-shot `didRestoreRef` guard;
`persistQueryClientRestore(options)` → `.then(onSuccess)` / `.catch(onError)` /
`.finally(setIsRestoring(false))`; while `isRestoring` the inner effect returns `undefined`
(no subscription); once false ⇒ `persistQueryClientSubscribe(options)` (effect deps
`[client, isRestoring]`). Wraps children in `QueryClientProvider` + `<IsRestoringProvider value={isRestoring}>`.
Package `index.ts` re-exports `* from '@tanstack/query-persist-client-core'` + the provider.

---

## 15. query-core exports frameworks rely on (must-not-miss list)

From `packages/query-core/src/index.ts`:

- **Classes/observers:** `QueryClient`, `QueryCache` (+ `QueryCacheNotifyEvent`), `MutationCache`
  (+ `MutationCacheNotifyEvent`), `MutationObserver`, `QueryObserver`, `InfiniteQueryObserver`,
  `QueriesObserver` (+ `QueriesObserverOptions`).
- **Hydration:** `dehydrate`, `hydrate`, `dehydrateQuery`, `defaultShouldDehydrateMutation`,
  `defaultShouldDehydrateQuery`; types `DehydratedState`, `DehydrateOptions`, `HydrateOptions`.
- **Managers (side-channel singletons):** `focusManager` (focused/visibility + subscribe),
  `onlineManager` (online + subscribe), `notifyManager` (`batch`, `batchCalls`, `schedule`,
  `scheduleMicrotask`, `setScheduler`), `timeoutManager` (`setTimeout/clearTimeout` with pluggable
  `TimeoutProvider`, `ManagedTimerId`) — persisters & throttles intentionally route timers through it,
  `environmentManager` (+ `isServer` deprecated shim).
- **Retryer:** `CancelledError`, `isCancelledError`.
- **Utils:** `hashKey`, `partialMatchKey`, `matchQuery`, `matchMutation`, `keepPreviousData`,
  `replaceEqualDeep`, `shouldThrowError`, `skipToken`, `noop`; types `QueryFilters`,
  `MutationFilters`, `SkipToken`, `Updater`.
- **Experimental:** `experimental_streamedQuery` (`streamedQuery(handler)` — queryFn that streams
  chunks into `onChunk`-style behavior).
- **Types:** `* from './types'` (all option/result types in §2–§4, `QueryStatus`, `FetchStatus`,
  `NotifyEventType` = `added|removed|updated|observerAdded|observerRemoved|observerResultsUpdated|
  observerOptionsUpdated`, `QueryState`, `MutationState`, `InvalidateQueryFilters{refetchType},
  RefetchOptions{cancelRefetch,throwOnError}, ResultOptions{throwOnError}`, `CancelOptions{revert,silent}`,
  `SetDataOptions{updatedAt}`, `QueryClientConfig{queryCache,mutationCache,defaultOptions}`,
  `DefaultOptions{queries,mutations,hydrate,dehydrate}`).
- **`QueryClient` method surface** (all used somewhere by adapters):
  `mount/unmount`, `getQueryCache/getMutationCache`, `getDefaultOptions/setDefaultOptions`,
  `getQueryDefaults/setQueryDefaults`, `getMutationDefaults/setMutationDefaults`,
  `defaultQueryOptions/defaultMutationOptions`, `fetchQuery|query`, `prefetchQuery`,
  `fetchInfiniteQuery|infiniteQuery`, `prefetchInfiniteQuery`, `ensureQueryData`,
  `ensureInfiniteQueryData` (deprecated-ish `FetchQueryOptions`/`EnsureQueryDataOptions` aliases),
  `getQueryData/getQueryState/getQueriesData/setQueryData/setQueriesData`,
  `removeQueries/resetQueries/cancelQueries/invalidateQueries/refetchQueries`,
  `isFetching/isMutating`, `resumePausedMutations`, `clear`.
- **`QueryState`** (what gets persisted): `data`, `dataUpdateCount`, `dataUpdatedAt`, `error`,
  `errorUpdateCount`, `errorUpdatedAt`, `fetchFailureCount`, `fetchFailureReason`, `fetchMeta`,
  `isInvalidated`, `status`, `fetchStatus`.

---

## 16. PORTING CHECKLIST — JS API → proposed Rust/GPUI equivalent

Legend: ✅ exists in `rs-query` today · 🟡 partial · ❌ missing. GPUI has no Suspense — the suspense
family is ported as *async-UI primitives* (loading-state holders + prefetch-before-render), not
render-blocking.

### A. Client / infrastructure

- [ ] `QueryClientProvider` → `QueryClient` as `gpui::Global` (or `Entity<QueryClient>` shared by
      clone) + `init_query_client(cx, client)` helper — ❌ (client exists, provider/global wiring ❌)
- [ ] `useQueryClient` → `client.clone()` from global/entity; explicit-arg override; panic/error if
      uninitialized — ❌
- [ ] `client.mount()/unmount()` (focus/online resubscribe) → attach on first provider init;
      GPUI: window-focus events via existing `FocusManager` — 🟡 (`focus_manager.rs` exists, no auto wiring)
- [ ] `onlineManager` → `trait ConnectivityMonitor` / `OnlineManager` entity polled by GPUI
      (network events) + `is_online()` — ❌
- [ ] `notifyManager.batchCalls/batch/schedule` → batched `cx.notify()`/`EventEmitter` emission
      (one wake per frame) — 🟡 (broadcast channels exist, no batching)
- [ ] `timeoutManager` → injectable timer source (async-std/tokio/GPUI timers) so throttles/GC/polling
      are testable — 🟡
- [ ] `focusManager` → ✅ (`FocusManager`); add visibility-change subscription to observers
- [ ] `QueryErrorResetBoundary` → `ErrorResetState` (shared `Entity<bool>`-like flag per UI region);
      hooks check `is_reset()` before rethrowing errors to UI — ❌ (low priority in GPUI; can fold
      into error display state)
- [ ] `isRestoring` context (`IsRestoringProvider`) → shared `Entity<RestoreState>` / flag on client:
      `client.is_restoring()`; observers skip auto-fetch while true — ❌
- [ ] `QueryClientConfig { queryCache, mutationCache, defaultOptions }` → `QueryClient::builder()`
      with `default_options: DefaultOptions { queries, mutations, hydrate, dehydrate }` — 🟡
- [ ] `defaultQueryOptions` dependent defaults (§2 step 4: `throwOnError = !!suspense`,
      `refetchOnReconnect = networkMode != always`, `persister ⇒ offlineFirst`, `skipToken ⇒ disabled`)
      → in `client.default_query_options()` — ❌
- [ ] Query filters (`QueryFilters{type, exact, predicate, queryKey, stale, fetchStatus}`,
      `InvalidateQueryFilters.refetchType`, `MutationFilters{exact, predicate, mutationKey, status}`)
      → `QueryFilter`/`MutationFilter` structs + `match_query/match_mutation` (key prefix vs exact) — ❌
      (rs-query has only `invalidate_queries(pattern)`)
- [ ] `CancelledError`/`isCancelledError` → `QueryError::Cancelled` — 🟡 (`cancellation.rs` exists)

### B. Core hooks

- [ ] `useQuery` → `spawn_query(client, key, fetch_fn, options)` + watched `QueryObserver` handle
      (GPUI entity with `cx.observe`) — 🟡 (`spawn_query` + `QueryObserver` exist; no per-hook
      optimistic result / tracking / subscribed flag)
  - [ ] option `queryKey` → `QueryKey` (✅; ensure stable `hash_key` incl. `partial_match_key`)
  - [ ] `queryFn` → async closure + `signal`-aware cancellation (🟡)
  - [ ] `enabled` (bool or fn) — 🟡 (bool only)
  - [ ] `staleTime` (incl. `'static'` + fn form) — 🟡 (Duration only)
  - [ ] `gcTime` — 🟡
  - [ ] `retry`/`retryDelay` — 🟡 (`RetryConfig`, add jitter + cap 30s)
  - [ ] `networkMode` + pause/resume (`isPaused`, `resumePausedMutations`) — ❌
  - [ ] `refetchInterval` (+`refetchIntervalInBackground`) — 🟡 (Duration only, no background flag)
  - [ ] `refetchOnWindowFocus/Reconnect/Mount` (bool | always | fn) — 🟡 (focus bool only)
  - [ ] `retryOnMount` — ❌
  - [ ] `notifyOnChangeProps` + prop tracking (`track_result`) → in GPUI: fine-grained entity fields or
        accept full-result notify; keep a `tracked: Option<Vec<Field>>` — ❌
  - [ ] `throwOnError` — ❌ (GPUI: surface as `result.error` + optional panic hook)
  - [ ] `select` — 🟡 (exists; add "run only when data changed / fn identity changed" memo)
  - [ ] `placeholderData` + `keepPreviousData` — 🟡 (`PlaceholderData::value/function`; add
        `keep_previous_data()` helper)
  - [ ] `initialData` + `initialDataUpdatedAt` — 🟡 (no `initial_data_updated_at`)
  - [ ] `meta` — ❌
  - [ ] `structuralSharing` — ✅ (`sharing.rs`; verify `replace_equal_deep` semantics + fn form)
  - [ ] `queryKeyHashFn` — ❌
  - [ ] `subscribed` — ❌ (equivalent: observer without auto-refresh)
  - [ ] result fields: add missing `dataUpdatedAt/errorUpdatedAt/errorUpdateCount/failureCount/
        failureReason/isFetched/isFetchedAfterMount/isLoading(isPending&&isFetching)/isPending/
        isLoadingError/isRefetchError/isPlaceholderData/isPaused/isEnabled/isStale/isRefetching` — 🟡
  - [ ] `refetch({cancelRefetch = true, throwOnError = false})` — 🟡 (`observer.refetch()`; add options)
- [ ] `useInfiniteQuery` → `spawn_infinite_query` + observer — 🟡
  - [ ] `initialPageParam` (required) — ✅
  - [ ] `getNextPageParam` / `getPreviousPageParam` — ✅ (`get_next_page_param`/`get_previous_page_param`)
  - [ ] `maxPages` (trim both directions) — 🟡
  - [ ] `hasNextPage/hasPreviousPage` — ✅
  - [ ] `fetchNextPage({cancelRefetch = true, throwOnError = false})` /
        `fetchPreviousPage({...})` — 🟡 (no options yet)
  - [ ] `isFetchingNextPage/isFetchingPreviousPage/isFetchNextPageError/isFetchPreviousPageError`
        via `state.fetchMeta` direction — ❌
  - [ ] `InfiniteData{pages, pageParams}` — ✅
  - [ ] refetch = restart from page 1 (≤ maxPages) — ❌
- [ ] `useMutation` → `spawn_mutation` + `MutationObserver` handle — 🟡
  - [ ] options: `mutationFn/mutationKey/onMutate/onSuccess/onError/onSettled/retry(0)/retryDelay/
        networkMode/gcTime/scope{id}/meta/throwOnError` — 🟡 (only `on_mutate/on_success/on_error` +
        `invalidates_key`; add `on_settled`, per-call callbacks, `scope`, `retry`, `gcTime`, `meta`)
  - [ ] `mutate(vars, {onSuccess, onError, onSettled})` per-call callbacks firing LAST, only while
        mounted (listener count > 0), options snapshotted — ❌
  - [ ] `mutateAsync` → `mutate_async(var) -> Result<TData>` (rejecting) — 🟡
  - [ ] `reset()` (detach + idle; doesn't cancel) — ❌
  - [ ] result/state: `context/data/error/failureCount/failureReason/isPaused/status{idle,pending,
        success,error}/variables/submittedAt` — 🟡 (state exists; add `context`, `submittedAt`,
        `failure_*`, `isPaused`)
  - [ ] exact side-effect order (§4.2: cache cb → option cb; settle order success: success→settled;
        error: error→settled; dispatch before/after) — ❌
  - [ ] MutationCache with `config.onMutate/onSuccess/onError/onSettled` + `findAll` + gc + scope
        queue (`canRun/runNext`) — ❌ (rs-query tracks counts only)
- [ ] `useQueries` → `QueriesObserver` entity holding `Vec<QueryObserver>` + `use_queries(cx, specs)`
      returning `Vec<QueryResult>` or combined via `combine: Box<dyn Fn(&[Result]) -> R>`; support
      dynamic length changes (`set_queries`) and top-level `subscribed`; per-query `placeholderData`
      gets `None` previous — ❌
- [ ] `useIsFetching(filters)` → `use_is_fetching` — 🟡 (exists, no filters; add
      `Omit<QueryFilters,'fetchStatus'>` incl. `type/exact/predicate/queryKey/stale`)
- [ ] `useIsMutating(filters)` → `use_is_mutating` — 🟡 (exists, no filters; forces `status: pending`)
- [ ] `useMutationState({filters, select})` → `use_mutation_state` — ❌ (needs MutationCache +
      findAll + `replace_equal_deep` snapshot caching)
- [ ] `skipToken` → `QueryFn::Skip` variant (or `Option<Arc<FetchFn>>`) forcing `enabled: false`;
      manual `refetch` rejected with `Missing queryFn` — ❌
- [ ] `queryOptions`/`infiniteQueryOptions`/`mutationOptions` helpers → not needed as identity fns in
      Rust (builders already are the pattern); optionally provide
      `query_options(key, fn).build()` shared structs — ➖

### C. Suspense / prefetch family (GPUI mapping: "load before render")

- [ ] `useSuspenseQuery` → `use_suspense_query` returning a guaranteed-data handle whose absent
      state renders the parent's loading placeholder; forces `enabled: true`,
      `throw_on_error: data == undefined`, no placeholder — ❌
- [ ] `useSuspenseInfiniteQuery` → same for infinite — ❌
- [ ] `useSuspenseQueries` → parallel variant (single combined "wait for all") — ❌
- [ ] `shouldSuspend` → `options.suspense && result.is_pending` — ❌
- [ ] `fetchOptimistic` (fetch w/o touching observer result; never rejects; clears reset boundary on
      error) → `observer.fetch_optimistic()` — ❌
- [ ] `ensureSuspenseTimers` (clamp staleTime/gcTime ≥ 1000 ms, keep `'static'`) → in
      `default_query_options` when suspense flag set — ❌
- [ ] `usePrefetchQuery` → `prefetch_query(client, options)`: skip if
      `client.get_query_state(key).is_some()`; fire-and-forget (log errors) — ❌
- [ ] `usePrefetchInfiniteQuery` → `prefetch_infinite_query(...)`: same guard; supports
      `{pages, getNextPageParam}` multi-page fetch — ❌
- [ ] `queryClient.query`/`infiniteQuery` (execute w/ `staleTime` fresh-return + `select` on the
      returned value) — ❌

### D. Hydration

- [ ] `dehydrate(client, {serializeData?, shouldDehydrateMutation?, shouldDehydrateQuery?,
      shouldRedactErrors?})` → `client.dehydrate(opts)` — 🟡 (exists, add options + defaults:
      success-queries-only, paused-mutations-only, error redaction)
- [ ] `hydrate(client, state, {defaultOptions:{deserializeData?, queries?, mutations?}})` — 🟡
      (exists; add newer-data-only merge, fetchStatus reset, pending→success promotion,
      `initialPromise` resume)
- [ ] `dehydrateQuery` payload incl. `dehydratedAt`, `promise`, `meta`, `queryType: 'infinite'` — ❌
- [ ] `HydrationBoundary` (render-time hydrate of new queries; effect hydrate of newer existing;
      no mutation defaultOptions) → hydrate at app/screen init: `HydrateOnMount(component, state)` — 🟡
      (plain hydrate exists; transition semantics N/A in GPUI — document as init-time)
- [ ] `DefaultOptions.dehydrate/.hydrate` passthrough — ❌

### E. Persistence (`query-persist-client-core` port)

- [ ] `Persister` → `trait QueryPersister`:
      `async persist_client(&self, client: PersistedClient)`, `async restore_client() ->
      Option<PersistedClient>`, `async remove_client()` — ❌
- [ ] `PersistedClient{timestamp, buster, clientState}` → same struct (`serde` serializable) — ❌
- [ ] `persistQueryClientRestore` → `persist_query_client_restore(&client, &persister, maxAge=24h,
      buster="", hydrate_options)`: expired/busted ⇒ remove; missing timestamp ⇒ remove; error ⇒
      remove + rethrow — ❌
- [ ] `persistQueryClientSave` → `persist_query_client_save`: dehydrate + persist with fresh
      `timestamp` — ❌
- [ ] `persistQueryClientSubscribe` → subscribe QueryCache **and** MutationCache; react only to
      `added|removed|updated` events; save errors logged — ❌ (needs cache event types in rs-query)
- [ ] `persistQueryClient` → `persist_query_client(...) -> (Unsubscribe, JoinHandle)`:
      restore-then-subscribe with unsubscribe race guard — ❌
- [ ] throttling lives in the persister (not in subscribe) — keep that split — ❌
- [ ] `removeOldestQuery` retryer → `remove_oldest_query` fn `(PersistedClient, error, errorCount) ->
      Option<PersistedClient>` (sort by dataUpdatedAt, drop oldest *query*, keep mutations, None when
      empty) — ❌
- [ ] `PersistRetryer` type → `type PersistRetryer = Arc<dyn Fn(...) -> Option<PersistedClient>>` — ❌
- [ ] `experimental_createQueryPersister` (fine-grained) → `create_query_persister(storage, opts)` — ❌
  - [ ] options: `storage/buster=''/maxAge=24h/serialize/deserialize/prefix='tanstack-query'/
        refetchOnRestore=true|'always'/filters` — ❌
  - [ ] storage keys `${prefix}-${query_hash}`; per-entry validation; expiry vs
        `state.dataUpdatedAt` — ❌
  - [ ] `persisterFn` as `QueryOptions.persister` hook (restore only if no data; post-restore
        setState of timestamps; refetch if stale) — ❌
  - [ ] `persist_query/persist_query_by_key/retrieve_query/persister_gc/restore_queries/remove_queries`
        (bulk ops need `entries()` on storage) — ❌
  - [ ] `AsyncStorage` trait with optional `entries()` — ❌
- [ ] distinction doc: whole-cache blob persister (§10.2, subscribe+throttle, timestamp-based maxAge)
      **vs** per-query persister (§10.3, queryFn wrapper, per-key entries, dataUpdatedAt-based
      maxAge, `networkMode: offlineFirst`) — keep both — ❌

### F. Storage persisters

- [ ] `createSyncStoragePersister` → `SyncStoragePersister::new(storage: Arc<dyn SyncKeyValue>,
      key: "REACT_QUERY_OFFLINE_CACHE", throttle_time: 1s, serialize/deserialize, retry)` — ❌
  - [ ] leading-edge throttle with latest-args via timeoutManager — ❌
  - [ ] retry loop on save failure (`trySave` while `error && client`) — ❌
  - [ ] null storage ⇒ all noops — ❌
- [ ] `createAsyncStoragePersister` → `AsyncStoragePersister::new(...)` (same options) — ❌
  - [ ] `asyncThrottle(interval, onError)`: never concurrent, latest args, spaced by
        `nextExecutionTime` — ❌
- [ ] `AsyncPersistRetryer` (Promisable return) — ❌

### G. Broadcast

- [ ] `broadcastQueryClient` → `broadcast_query_client(client, channel_name="tanstack-query") ->
      Unsubscribe` — ❌
  - [ ] GPUI analogue: intra-app multi-window sync via `gpui::EventEmitter`/`Global` bus; true
        cross-process needs IPC (document both) — ❌
  - [ ] broadcast only: updated-on-success, removed-with-observers, added — ❌
  - [ ] apply: updated ⇒ setState-or-build; removed ⇒ remove; added ⇒ setState only if no data — ❌
  - [ ] re-entrancy guard (`transaction` flag with guaranteed reset) — ❌
  - [ ] `onBroadcastError` (serialization failures) → callback + dev warn — ❌

### H. Misc framework-agnostic extras

- [ ] `hashKey`/`partialMatchKey` — 🟡 (key.rs has cache_key; add structured deep-hash + prefix match)
- [ ] `matchQuery`/`matchMutation` — ❌
- [ ] `keepPreviousData` → `PlaceholderData::keep_previous_data()` — ❌ (helper)
- [ ] `replaceEqualDeep` — 🟡 (sharing.rs)
- [ ] `shouldThrowError` → helper for `throwOnError` bool|fn resolution — ❌
- [ ] `experimental_streamedQuery` → skip initially; possible `streamed_query` helper writing chunks — ➖
- [ ] `NotifyEventType` taxonomy on QueryCache/MutationCache (`added/removed/updated/observer*`) — ❌
      (needed by persist subscribe, useIsFetching, useMutationState, broadcast)
- [ ] `QueryState` full shape incl. `fetchMeta`, `isInvalidated`, update counters — 🟡
- [ ] devtools (✅ `devtools.rs`) — extend once cache events land

---

## 17. Quick-reference: literal constants

| Constant | Value |
|---|---|
| `persistQueryClientRestore.maxAge` | `24 * 60 * 60 * 1000` |
| `persistQueryClientRestore.buster` | `''` |
| `persistQueryClientSubscribe` save triggers | cache events `added`, `removed`, `updated` (both caches) |
| `createSyncStoragePersister.key` | `'REACT_QUERY_OFFLINE_CACHE'` |
| `createSyncStoragePersister.throttleTime` | `1000` (ms, leading-edge) |
| `createAsyncStoragePersister.throttleTime` | `1000` (ms, asyncThrottle interval) |
| `PERSISTER_KEY_PREFIX` | `'tanstack-query'` |
| `broadcastQueryClient.broadcastChannel` | `'tanstack-query'` |
| `MIN_SUSPENSE_TIME_MS` | `1000` (staleTime/gcTime clamp under suspense) |
| default `gcTime` | `5 * 60 * 1000` (5 min; `Infinity` on server) |
| default `retry` | queries `3` (client) / `0` (server); mutations `0` |
| default `retryDelay` | exponential backoff capped at 30s |
| storage-persister default `serialize`/`deserialize` | `JSON.stringify` / `JSON.parse` |
| dehydrate defaults | successful queries only; paused mutations only; errors redacted to `Error('redacted')` |
