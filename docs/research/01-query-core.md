# 01 — TanStack `query-core` Public API Inventory (for rs-query port)

> Source audited: `/home/z/my-project/tanstack-query/packages/query-core/src` — package
> `@tanstack/query-core` **v5.105.0** (single entrypoint `src/index.ts`, `export * from './types'`).
> Everything below was read directly from source. Types are given as TS names; Rust hints use the
> snake_case name and point at existing rs-query modules (`client.rs`, `query.rs`, `observer.rs`,
> `mutation.rs`, `hydration.rs`, `sharing.rs`, `key.rs`, `cancellation.rs`, `executor.rs`,
> `infinite.rs`, `state.rs`, `focus_manager.rs`, `error.rs`).

Two names from the porting plan that **do not exist** in query-core v5 (do not port):
- **`Logger` / `setLogger`** — removed in v5. Errors go straight to `console.error` in dev paths.
- **`HydrateObserver`** — does not exist. Hydration is plain functions in `hydration.ts`
  (`hydrate`, `dehydrate`, `dehydrateQuery`). React uses `HydrationBoundary`, not an observer.
- **`notifyManager.scheduleMicrotask`** — that was v4. v5 has `schedule` + `setScheduler`
  (default `setTimeout(cb, 0)` via `systemSetTimeoutZero`).
- **`QueryPageParams`** — does not exist; the real types are `InitialPageParam` and
  `InfiniteQueryPageParamsOptions`.
- **`queryClient.isRestoring`** — does not exist. "Restoring" is expressed as
  `options._optimisticResults = 'isRestoring'` (observer side, set by framework adapters) and forces
  `fetchStatus: 'idle'` in `createResult`.

---

## 1. Module map (files → what they own)

| File | Exports |
|---|---|
| `index.ts` | re-export surface (§2) |
| `types.ts` | all option/result/type declarations (§6–§9) |
| `queryClient.ts` | `QueryClient` |
| `queryCache.ts` | `QueryCache`, `QueryCacheConfig`, `QueryCacheNotifyEvent`, `QueryStore` |
| `mutationCache.ts` | `MutationCache`, `MutationCacheConfig`, `MutationCacheNotifyEvent` |
| `query.ts` | `Query`, `QueryState`, `FetchContext`, `QueryBehavior`, `FetchDirection`, `FetchMeta`, `FetchOptions`, `Action`, `fetchState()` |
| `mutation.ts` | `Mutation`, `MutationState`, `Action`, `getDefaultState()` |
| `queryObserver.ts` | `QueryObserver` (+ private `ObserverFetchOptions`, mount/refetch predicates) |
| `infiniteQueryObserver.ts` | `InfiniteQueryObserver` |
| `infiniteQueryBehavior.ts` | `infiniteQueryBehavior(pages?)`, `hasNextPage()`, `hasPreviousPage()` (not re-exported from index) |
| `mutationObserver.ts` | `MutationObserver` |
| `queriesObserver.ts` | `QueriesObserver`, `QueriesObserverOptions` |
| `subscribable.ts` | `Subscribable<TListener>` |
| `removable.ts` | `Removable` (abstract GC base) |
| `retryer.ts` | `createRetryer`, `Retryer`, `RetryerConfig`, `CancelledError`, `isCancelledError`, `canFetch`, `RetryValue`, `RetryDelayValue` |
| `utils.ts` | `QueryFilters`, `MutationFilters`, `QueryTypeFilter`, `Updater`, `SkipToken`/`skipToken`, `hashKey`, `partialMatchKey`, `matchQuery`, `matchMutation`, `replaceEqualDeep`, `replaceData`, `keepPreviousData`, `shallowEqualObjects`, `isPlainObject`, `isPlainArray`, `functionalUpdate`, `resolveQueryValue`, `timeUntilStale`, `isValidTimeout`, `noop`, `sleep`, `ensureQueryFn`, `shouldThrowError`, `addConsumeAwareSignal`, `addToEnd`, `addToStart`, `hashQueryKeyByOptions`, `isServer` (deprecated const) |
| `hydration.ts` | `dehydrate`, `dehydrateQuery`, `hydrate`, `defaultShouldDehydrateMutation`, `defaultShouldDehydrateQuery`, `DehydratedState`, `DehydratedQuery`, `DehydratedMutation`, `DehydrateOptions`, `HydrateOptions` |
| `focusManager.ts` | `focusManager`, `FocusManager` |
| `onlineManager.ts` | `onlineManager`, `OnlineManager` |
| `notifyManager.ts` | `notifyManager`, `createNotifyManager`, `defaultScheduler` |
| `timeoutManager.ts` | `timeoutManager`, `TimeoutManager`, `TimeoutProvider`, `defaultTimeoutProvider`, `ManagedTimerId`, `TimeoutCallback`, `systemSetTimeoutZero` |
| `environmentManager.ts` | `environmentManager` (`isServer()`, `setIsServer()`) |
| `streamedQuery.ts` | `streamedQuery` (exported as `experimental_streamedQuery`) |
| *(adjacent package)* `query-persist-client-core` | `experimental_createQueryPersister`, `persistQueryClient*`, `removeOldestQuery` (§12) |

---

## 2. Exact export surface of `index.ts`

Value exports: `focusManager`, `environmentManager`, `defaultShouldDehydrateMutation`,
`defaultShouldDehydrateQuery`, `dehydrate`, `dehydrateQuery`, `hydrate`, `InfiniteQueryObserver`,
`MutationCache`, `MutationObserver`, `defaultScheduler`, `notifyManager`, `onlineManager`,
`QueriesObserver`, `QueryCache`, `QueryClient`, `QueryObserver`, `CancelledError`,
`isCancelledError` (deprecated), `timeoutManager`, `hashKey`, `isServer` (deprecated),
`keepPreviousData`, `matchMutation`, `matchQuery`, `noop`, `partialMatchKey`, `replaceEqualDeep`,
`shouldThrowError`, `skipToken`, `experimental_streamedQuery`, `Mutation`, `Query`,
plus `export * from './types'`.

Type exports: `FocusManager`, `MutationCacheNotifyEvent`, `OnlineManager`, `QueryCacheNotifyEvent`,
`ManagedTimerId`, `TimeoutCallback`, `TimeoutManager`, `TimeoutProvider`, `MutationFilters`,
`QueryFilters`, `SkipToken`, `Updater`, `DehydratedState`, `DehydrateOptions`, `HydrateOptions`,
`MutationState`, `MutationCacheConfig`, `QueriesObserverOptions`, `QueryState`, `QueryCacheConfig`,
and everything in §6–§9.

---

## 3. `QueryClient` — complete method inventory

```ts
class QueryClient {
  constructor(config?: QueryClientConfig)   // { queryCache?, mutationCache?, defaultOptions? }
}
```

Private state: `#queryCache`, `#mutationCache`, `#defaultOptions`, `#queryDefaults: Map<string, QueryDefaults>`, `#mutationDefaults: Map<string, MutationDefaults>`, `#mountCount`, focus/online unsubscribe fns.

| Method | Signature | Exact behavior |
|---|---|---|
| `mount()` | `() => void` | Ref-counted (`#mountCount++`, act only when count becomes 1). Subscribes focusManager: on focus → `await resumePausedMutations()` then `queryCache.onFocus()`. Subscribes onlineManager: on online → `await resumePausedMutations()` then `queryCache.onOnline()`. |
| `unmount()` | `() => void` | `#mountCount--`; tears down both subscriptions only at 0. |
| `isFetching` | `(filters?: Omit<QueryFilters,'fetchStatus'>) => number` | `queryCache.findAll({...filters, fetchStatus:'fetching'}).length`. |
| `isMutating` | `(filters?: MutationFilters) => number` | `mutationCache.findAll({...filters, status:'pending'}).length`. |
| `getQueryData` | `(queryKey) => data \| undefined` | `defaultQueryOptions({queryKey})` → `queryCache.get(queryHash)?.state.data`. No query created. |
| `setQueryData` | `(queryKey, updater: Updater<data\|undefined>, options?: SetDataOptions) => data \| undefined` | `functionalUpdate(updater, prev)`; if result `undefined` → no-op, returns `undefined` (query NOT created). Else `queryCache.build(...).setData(data, {...options, manual:true})`. |
| `getQueriesData` | `(filters) => Array<[QueryKey, data\|undefined]>` | `queryCache.findAll(filters).map(q => [q.queryKey, q.state.data])`. |
| `setQueriesData` | `(filters, updater, options?) => Array<[QueryKey, data\|undefined]>` | `notifyManager.batch` over matched queries calling `setQueryData` per key. Only existing+matched queries updated. |
| `getQueryState` | `(queryKey) => QueryState \| undefined` | `queryCache.get(hash)?.state`. |
| `fetchQuery` ⚠ deprecated (→ `query()`) | `(options: FetchQueryOptions) => Promise<TData>` | Defaults options; **forces `retry = false` when `retry === undefined`**; build; `isStaleByTime(staleTime)` ? `query.fetch(options)` : cached data. |
| `prefetchQuery` ⚠ deprecated | `(options) => Promise<void>` | `fetchQuery(options).then(noop).catch(noop)` — never rejects. |
| `ensureQueryData` ⚠ deprecated | `(options: EnsureQueryDataOptions) => Promise<TData>` | cached data undefined → `fetchQuery`; else if `options.revalidateIfStale && query.isStaleByTime(staleTime)` → `void prefetchQuery(defaulted)` (background); resolves cached data. |
| `query` (new, replaces fetchQuery/ensureQueryData) | `async (options: QueryExecuteOptions) => Promise<TData>` | Same retry-forcing; `isStale` ? `await query.fetch()` : cached; **applies `select` to the returned value in both cases** (does not affect cache). |
| `infiniteQuery` (new) | `(options: InfiniteQueryExecuteOptions) => Promise<InfiniteData>` | Sets `options._type = 'infinite'`, delegates to `query`. Extra: `initialPageParam` (required), optional `pages: number` + `getNextPageParam` to fetch N pages from the start. |
| `fetchInfiniteQuery` ⚠ deprecated | | `options._type='infinite'` then `fetchQuery`. |
| `prefetchInfiniteQuery` ⚠ deprecated | | `fetchInfiniteQuery(...).then(noop).catch(noop)`. |
| `ensureInfiniteQueryData` ⚠ deprecated | | `options._type='infinite'` then `ensureQueryData`. |
| `invalidateQueries` | `(filters?: InvalidateQueryFilters, options?: InvalidateOptions = {}) => Promise<void>` | In batch: `findAll(filters).forEach(q => q.invalidate())`; if `filters.refetchType === 'none'` → resolve; else `refetchQueries({...filters, type: filters?.refetchType ?? filters?.type ?? 'active'}, options)`. Invalidation itself does NOT refetch — the follow-up call does. |
| `refetchQueries` | `(filters?: RefetchQueryFilters, options: RefetchOptions = {}) => Promise<void>` | `fetchOptions.cancelRefetch ??= true`; in batch: matched, **filtered to `!q.isDisabled() && !q.isStatic()`**, mapped to `q.fetch(undefined, fetchOptions)`; without `throwOnError` each promise gets `.catch(noop)`; **queries whose `fetchStatus === 'paused'` contribute an already-resolved promise**; `Promise.all(...).then(noop)`. |
| `resetQueries` | `(filters?, options?: ResetOptions) => Promise<void>` | In batch: `findAll(filters)` → `q.reset()` for each; then `refetchQueries({type:'active', predicate: q => matchedSet.has(q)}, options)`. |
| `cancelQueries` | `(filters?, cancelOptions: CancelOptions = {}) => Promise<void>` | `defaultedCancelOptions = { revert: true, ...cancelOptions }`; in batch: cancel every matched query; `Promise.all(promises).then(noop).catch(noop)` — **never rejects**. |
| `removeQueries` | `(filters?) => void` | In batch: `findAll(filters).forEach(q => queryCache.remove(q))`. |
| `resumePausedMutations` | `() => Promise<unknown>` | If `onlineManager.isOnline()` → `mutationCache.resumePausedMutations()`; else `Promise.resolve()`. |
| `getQueryCache` / `getMutationCache` | `() => QueryCache / MutationCache` | accessors. |
| `getDefaultOptions` | `() => DefaultOptions` | as set at construction / by `setDefaultOptions`. |
| `setDefaultOptions` | `(options: DefaultOptions) => void` | **Replaces** the whole default options object. |
| `setQueryDefaults` | `(queryKey, options: Partial<QueryObserverOptions w/o queryKey>) => void` | `#queryDefaults.set(hashKey(queryKey), {queryKey, defaultOptions})`. |
| `getQueryDefaults` | `(queryKey) => options` | Merges **all** registered defaults whose key `partialMatchKey`-matches, **in registration order** (register generic → specific). |
| `setMutationDefaults` / `getMutationDefaults` | same pattern with `mutationKey` | Same merge semantics. |
| `defaultQueryOptions` | `(options) => DefaultedQueryObserverOptions` | If `options._defaulted` → return as-is. Else `{...defaultOptions.queries, ...getQueryDefaults(queryKey), ...options, _defaulted: true}`; computes `queryHash` if missing (`hashQueryKeyByOptions`); **dependent defaults**: `refetchOnReconnect ??= networkMode !== 'always'`; `throwOnError ??= !!suspense`; `networkMode ??= persister ? 'offlineFirst' : undefined`; `queryFn === skipToken → enabled = false`. |
| `defaultMutationOptions` | `(options?) => options` | If `_defaulted` → as-is; else `{...defaultOptions.mutations, ...(mutationKey && getMutationDefaults(mutationKey)), ...options, _defaulted: true}`. |
| `clear` | `() => void` | `queryCache.clear(); mutationCache.clear()`. |

`DefaultOptions`: `{ queries?: OmitKeyof<QueryObserverOptions, 'suspense'|'queryKey'>, mutations?: MutationObserverOptions<unknown, DefaultError, unknown, unknown>, hydrate?: HydrateOptions['defaultOptions'], dehydrate?: DehydrateOptions }`.

---

## 4. `QueryCache`

```ts
class QueryCache extends Subscribable<(event: QueryCacheNotifyEvent) => void> {
  constructor(config?: QueryCacheConfig)  // { onError?, onSuccess?, onSettled? } — fire-and-forget, not awaited
}
```
Storage: `Map<queryHash, Query>` (pluggable via `QueryStore` interface: has/set/get/delete/values).

| Method | Behavior |
|---|---|
| `build(client, options, state?)` | Get-or-create: hash = `options.queryHash ?? hashQueryKeyByOptions(queryKey, options)`; new queries get `client.defaultQueryOptions(options)` and `client.getQueryDefaults(queryKey)` as `defaultOptions`; existing query returned unchanged (state ignored). |
| `add(query)` | No-op if hash present; else insert + notify `{type:'added', query}`. |
| `remove(query)` | Only if the stored query under `query.queryHash` **is** `query`: `query.destroy()`, delete, notify `{type:'removed'}`. |
| `clear()` | batch: remove every query. |
| `get(queryHash)` | direct map lookup. |
| `getAll()` | `[...map.values()]`. |
| `find(filters)` | `defaultedFilters = { exact: true, ...filters }` (requires `queryKey`); first `matchQuery` hit. |
| `findAll(filters = {})` | `filters` empty → all queries; else `matchQuery` filter. |
| `notify(event)` | `notifyManager.batch(() => listeners.forEach(l => l(event)))`. |
| `onFocus()` / `onOnline()` | batch: `query.onFocus()` / `query.onOnline()` for every query. |

`QueryCacheNotifyEvent` = `{type:'added',query}` | `{type:'removed',query}` | `{type:'updated',query,action}` | `{type:'observerAdded',query,observer}` | `{type:'observerRemoved',query,observer}` | `{type:'observerResultsUpdated',query}` | `{type:'observerOptionsUpdated',query,observer}`.

## 5. `MutationCache`

```ts
class MutationCache extends Subscribable<(event: MutationCacheNotifyEvent) => void> {
  constructor(config?: MutationCacheConfig)
  // config callbacks ARE awaited before the mutation continues:
  // onMutate(variables, mutation, context), onSuccess(data, variables, onMutateResult, mutation, context),
  // onError(error, variables, onMutateResult, mutation, context),
  // onSettled(data|undefined, error|null, variables, onMutateResult, mutation, context)
}
```
Storage: `Set<Mutation>` + `Map<scopeId, Mutation[]>` + monotonically increasing `#mutationId`.

| Method | Behavior |
|---|---|
| `build(client, options, state?)` | **Always creates a new Mutation** (`mutationId: ++counter`, options defaulted via `client.defaultMutationOptions`), adds, returns. |
| `add(mutation)` | Insert; if `options.scope?.id` is a string, append to scope queue; notify `{type:'added'}`. |
| `remove(mutation)` | Delete from set; splice from scope queue (delete scope when last); **always notifies `{type:'removed'}` even if it wasn't in the set**. |
| `canRun(mutation)` | Scope semantics: unscoped → `true`; scoped → `true` only if no pending mutation in the scope, or `this` is the first pending one. |
| `runNext(mutation)` | After a mutation finishes, `continue()` the next paused mutation in the same scope (returns its promise or `Promise.resolve()`). |
| `clear()` | batch: clear set + scopes, notify `removed` per mutation. |
| `getAll()` / `find(filters)` / `findAll(filters)` | like QueryCache; `find` defaults `exact: true`. |
| `notify(event)` | batch listeners. |
| `resumePausedMutations()` | batch: `Promise.all(paused.map(m => m.continue().catch(noop)))`. |

`MutationCacheNotifyEvent` types: `added`, `removed`, `updated` (+action), `observerAdded`, `observerRemoved`, `observerOptionsUpdated` (note: **no** `observerResultsUpdated`).

---

## 6. `Query` class

```ts
class Query<TQueryFnData, TError, TData, TQueryKey> extends Removable {
  queryKey: TQueryKey
  queryHash: string
  options: QueryOptions          // setOptions merges over per-query defaultOptions
  state: QueryState<TData, TError>
  observers: Array<QueryObserver>
  get meta(): QueryMeta | undefined          // this.options.meta
  get queryType(): 'infinite' | undefined    // from options._type
  get promise(): Promise<TData> | undefined  // current retryer promise; undefined when idle
  get resetState(): QueryState               // the state the query was created with (initial state incl. initialData)
}
```

`QueryState` (raw cache state, exported):
`data, dataUpdateCount, dataUpdatedAt, error, errorUpdateCount, errorUpdatedAt, fetchFailureCount, fetchFailureReason, fetchMeta: FetchMeta | null, isInvalidated, status: QueryStatus, fetchStatus: FetchStatus`.

| Method | Behavior |
|---|---|
| `setOptions(options?)` | `this.options = {...perQueryDefaults, ...options}`; records `_type`; `updateGcTime(options.gcTime)`; **if state exists and has no data, applies `initialData` from the new options as a success state** and updates the stored initial state. |
| `setData(newData, options?: SetDataOptions & {manual: boolean})` | `data = replaceData(this.state.data, newData, this.options)` (structural sharing); dispatch `{type:'success', data, dataUpdatedAt: options?.updatedAt, manual}`; returns written data. |
| `setState(state: Partial<QueryState>)` | dispatch `{type:'setState', state}` (merge). Used by devtools/persist/broadcast. |
| `cancel(options?: CancelOptions)` | `retryer.cancel(options)`; resolves after the in-flight promise settles (`.then(noop).catch(noop)`); resolves immediately when idle. |
| `destroy()` | `super.destroy()` (clear GC timer) **+ `cancel({silent: true})`**. |
| `reset()` | `destroy()` then `setState(this.resetState)` — back to initial state (initialData re-applied), NOT removed from cache. |
| `invalidate()` | If not already invalidated: dispatch `{type:'invalidate'}` (`isInvalidated = true`). No fetch. |
| `isActive()` | `observers.some(o => resolveQueryValue(o.options.enabled, this) !== false)`. |
| `isDisabled()` | With observers → `!isActive()`; without → `queryFn === skipToken \|\| !isFetched()`. |
| `isFetched()` | `dataUpdateCount + errorUpdateCount > 0`. |
| `isStatic()` | Any observer with `staleTime === 'static'`. |
| `isStale()` | With observers → `observers.some(o => o.getCurrentResult().isStale)`; without → `data === undefined \|\| isInvalidated`. |
| `isStaleByTime(staleTime: StaleTime = 0)` | no data → true; `'static'` → false; `isInvalidated` → true; else `timeUntilStale(dataUpdatedAt, staleTime) === 0`. |
| `onFocus()` / `onOnline()` | `observers.find(x => x.shouldFetchOnWindowFocus()/shouldFetchOnReconnect())?.refetch({cancelRefetch:false})`; **plus `retryer.continue()`** (resume paused fetch). |
| `addObserver(o)` | push if absent, `clearGcTimeout()`, notify `observerAdded`. |
| `removeObserver(o)` | splice; **if last observer**: if `abortSignalConsumed \|\| (fetchStatus==='paused' && status==='pending')` → `retryer.cancel({revert:true})` else `retryer.cancelRetry()`; then `scheduleGc()`; notify `observerRemoved`. |
| `getObserversCount()` | `observers.length`. |
| `fetch(options?, fetchOptions?: FetchOptions)` | see below. |
| (protected) `optionalRemove()` | GC: remove from cache only when `!observers.length && state.fetchStatus === 'idle'`. |

`FetchOptions` = `{ cancelRefetch?: boolean, meta?: FetchMeta, initialPromise?: Promise<TData> }`; `FetchMeta = { fetchMore?: { direction: FetchDirection } }`.

`fetch()` exact algorithm (critical to port faithfully):
1. If `state.fetchStatus !== 'idle'` and `retryer.status() !== 'rejected'` (dedup):
   - `data !== undefined && fetchOptions?.cancelRefetch` → `cancel({silent:true})` (fall through to a new fetch),
   - else → `retryer.continueRetry()` and **return the existing retryer promise** (fetch sharing).
2. `if (options) setOptions(options)` — options NOT applied when reusing an in-flight fetch.
3. If `!options.queryFn` → adopt options from the **first observer that has a `queryFn`** (hydrated / setQueryData-created queries).
4. dev-only check that `queryKey` is an array.
5. New `AbortController` per fetch. `signal` is added to both the `QueryFunctionContext` and the `FetchContext` as an **enumerated lazy getter**; first read sets `#abortSignalConsumed = true`.
6. `fetchFn` = `ensureQueryFn(options, fetchOptions)` wrapped: context `{client, queryKey, meta}` (+lazy signal); if `options.persister` → `persister(queryFn, context, query)` else `queryFn(context)`.
7. Behavior hook: `queryType === 'infinite'` → `infiniteQueryBehavior(options.pages)` else `options.behavior`; `behavior.onFetch(context, query)` may replace `context.fetchFn`.
8. `#revertState = state`; dispatch `{type:'fetch', meta}` **only if** `fetchStatus === 'idle'` or `fetchMeta` differs from `fetchOptions.meta`.
9. `createRetryer({ initialPromise: fetchOptions?.initialPromise, fn: context.fetchFn, onCancel: error => { if (error is CancelledError && error.revert) setState({...revertState, fetchStatus:'idle'}); abortController.abort() }, onFail: (failureCount, error) => dispatch 'failed', onPause: () => dispatch 'pause', onContinue: () => dispatch 'continue', retry, retryDelay, networkMode, canRun: () => true })`.
10. `await retryer.start()`; result `undefined` → dev console.error + throw `Error('${queryHash} data is undefined')`; else `setData(data)`; then cache config `onSuccess`/`onSettled` (return value ignored); return data.
11. Catch: `CancelledError.silent` → **return `this.#retryer.promise`** (piggyback on the replacing fetch); `CancelledError.revert` → if `state.data === undefined` rethrow else **return `state.data`**; otherwise dispatch `{type:'error'}` (error, `errorUpdateCount+1`, `errorUpdatedAt: now`, `fetchFailureCount+1`, `fetchFailureReason`, `fetchStatus:'idle'`, `status:'error'`, **`isInvalidated: true`**), cache config `onError`/`onSettled`, rethrow.
12. `finally`: if `#retryer === retryer` → `#retryer = undefined` (don't pin result); `scheduleGc()`.

State reducer (exported helper `fetchState(data, options)`):
- `fetch`: `{fetchFailureCount:0, fetchFailureReason:null, fetchStatus: canFetch(networkMode) ? 'fetching' : 'paused', ...(data === undefined && {error: null, status: 'pending'})}`
- `success`: `{...successState(data, dataUpdatedAt), dataUpdateCount+1, ...(!manual && {fetchStatus:'idle', fetchFailureCount:0, fetchFailureReason:null})}`; `revertState = manual ? newState : undefined`; `successState` = `{data, dataUpdatedAt: dataUpdatedAt ?? Date.now(), error:null, isInvalidated:false, status:'success'}`
- `error`: as above (sets `isInvalidated: true` — background errors mark data stale)
- `pause`/`continue`: `fetchStatus` ⇄ `'paused'`/`'fetching'`
- `failed`: `{fetchFailureCount, fetchFailureReason}` from retryer
- `invalidate`: `isInvalidated: true`; `setState`: merge partial.

`getDefaultState(options)` (initial): `initialData` resolved (fn called **once**); `hasData` → `status:'success'`, `dataUpdatedAt: initialDataUpdatedAt?.() ?? Date.now()`; else `status:'pending'`, `dataUpdatedAt: 0`; everything else zeroed; `fetchStatus:'idle'`.

Dispatch notifies: `observers.slice().forEach(o => o.onQueryUpdate())` (copy to survive unsubscribe during iteration) + cache `notify({type:'updated', action})`, all inside `notifyManager.batch`.

---

## 7. `Mutation` class

```ts
class Mutation<TData, TError, TVariables, TOnMutateResult> extends Removable {
  state: MutationState<...>   // { context, data, error, failureCount, failureReason, isPaused, status, variables, submittedAt }
  options: MutationOptions
  readonly mutationId: number
  get meta(): MutationMeta | undefined
}
```

| Method | Behavior |
|---|---|
| `setOptions(options)` | Replaces options outright; `updateGcTime(options.gcTime)`. |
| `addObserver(o)` / `removeObserver(o)` | push/filter + gc stop/start + cache notify `observerAdded`/`observerRemoved`. |
| (protected) `optionalRemove()` | No observers: still `pending` → `scheduleGc()` again (defer); else `mutationCache.remove(this)`. **Pending mutations are never gc'ed while running.** |
| `continue()` | `retryer?.continue()`; else if `status === 'pending'` (restored from dehydrate, never had a retryer here) → `execute(state.variables!)`; else resolve immediately (settled). |
| `execute(variables)` | Full lifecycle below. |

`execute()` exact ordering:
1. `mutationFnContext = {client, meta: options.meta, mutationKey: options.mutationKey}`.
2. `createRetryer({ fn: () => mutationFn ? mutationFn(variables, ctx) : reject('No mutationFn found'), onFail/onPause/onContinue dispatch, retry: options.retry ?? 0, retryDelay, networkMode, canRun: () => mutationCache.canRun(this) })` — scope queuing happens **inside the retryer's `canRun`**.
3. `restored = state.status === 'pending'`; `isPaused = !retryer.canStart()`.
4. If `restored` → dispatch `{type:'continue'}` (unpause) skipping `onMutate`; else dispatch `{type:'pending', variables, isPaused}` → **await `mutationCache.config.onMutate`** → **await `options.onMutate`** → if context changed, dispatch `{type:'pending', context, variables, isPaused}`.
5. `await retryer.start()`.
6. Success path (each awaited, in order): `cache.onSuccess` → `options.onSuccess(data, variables, state.context!, ctx)` → `cache.onSettled(data, null, ...)` → `options.onSettled(data, null, ...)` → dispatch `{type:'success', data}` → return data.
7. Error path: each of `cache.onError`, `options.onError`, `cache.onSettled(undefined, error, ...)`, `options.onSettled(undefined, error, ...)` is **individually try/caught** (a throwing callback becomes an unhandled rejection, never masks the original error); dispatch `{type:'error'}` (`data: undefined`, `failureCount+1`, `failureReason`, `isPaused:false`, `status:'error'`); **rethrow original error**.
8. `finally`: drop settled retryer reference; `mutationCache.runNext(this)` (start next scoped mutation).

Reducer `pending` action resets `data: undefined, failureCount: 0, failureReason: null, error: null`, sets `variables`, `submittedAt: Date.now()`. `MutationState` is what dehydrate serializes. Exported `getDefaultState()` = all-empty, `status:'idle'`, `isPaused:false`, `submittedAt: 0`.

---

## 8. Observers

### 8.1 `QueryObserver` (`extends Subscribable<result => void>`)

Constructor: `new QueryObserver(client, options)` — options are defaulted immediately via `setOptions`.

Public API:
| Member | Notes |
|---|---|
| `options` | current (defaulted) options. |
| `setOptions(options)` | defaults via `client.defaultQueryOptions`; **throws `Error('Expected enabled to be a boolean or a callback that returns a boolean')`** if `enabled` is not bool/fn; `#updateQuery()` (switch query on hash change); `currentQuery.setOptions(this.options)`; if prev was `_defaulted` and options changed shallowly → cache notify `observerOptionsUpdated`; if mounted and `shouldFetchOptionally(...)` → `#executeFetch()`; `updateResult()`; reschedule stale timeout if query/enabled/staleTime changed; recompute + reschedule refetch interval. |
| `getOptimisticResult(defaultedOptions)` | builds the query, `createResult` now; if result differs from `getCurrentResult()` it **mutates observer state** (`#currentResult`, `#currentResultOptions`, `#currentResultState`) — the "optimistic result cursor" trick. Called by adapters each render before subscribing. |
| `getCurrentResult()` | last computed result. |
| `trackResult(result, onPropTracked?)` | Proxy recording property reads → `trackProp`. |
| `trackProp(key)` | adds to `#trackedProps` (drives default notify filtering). |
| `getCurrentQuery()` | the observed `Query`. |
| `refetch(options?: RefetchOptions = {})` | `this.fetch({...options})` — `cancelRefetch` defaults true inside `fetch`. Returns promise of the *new result*. |
| `fetchOptimistic(options)` | prefetch another query without touching this observer: race between `query.fetch(...)` and a cache `'updated'` subscription; resolves with `createResult` of that query. |
| `shouldFetchOnWindowFocus()` / `shouldFetchOnReconnect()` | `shouldFetchOn(query, options, options.refetchOnWindowFocus / refetchOnReconnect)`. |
| `destroy()` | clears listeners, stale timeout, refetch interval, removes self from query. |
| `onQueryUpdate()` | (called by `Query.#dispatch`) `updateResult()` + reschedule timers if mounted. |
| (protected) `fetch(fetchOptions: ObserverFetchOptions)` | `#executeFetch({...fetchOptions, cancelRefetch ?? true}).then(() => { updateResult(); return currentResult })`. `ObserverFetchOptions = FetchOptions & { throwOnError?: boolean }` (not exported). |
| (protected) `createResult(query, options)` | result computation (below). |
| (protected) `updateResult()` | recompute; skip entirely if `shallowEqualObjects(next, prev)`; else set `#currentResult`; notify decision (below); then cache notify `observerResultsUpdated` — in one `notifyManager.batch`, listeners first. |
| (protected) `onSubscribe` | first listener: `currentQuery.addObserver(this)`; `shouldFetchOnMount` → `#executeFetch()` else `updateResult()`; `#updateTimers()`. |
| (protected) `onUnsubscribe` | no listeners → `destroy()`. |

`createResult` details worth porting:
- `_optimisticResults` set → optimistic overlay `fetchState(state.data, query.options)` when (unmounted & shouldFetchOnMount) or (mounted & shouldFetchOptionally); `'isRestoring'` additionally forces `fetchStatus = 'idle'`.
- `placeholderData`: used only when `data === undefined && status === 'pending'`; **memoized** when previous result `isPlaceholderData` and same `placeholderData` option (select skipped then); function form receives `(lastQueryWithDefinedData.state.data, lastQueryWithDefinedData)`; result passes through `replaceData(prevResult?.data, placeholderData, options)`; sets `status:'success'`, `isPlaceholderData: true`.
- `select`: runs only if `data !== undefined`; memoized on `data === prevResultState.data && select === previousSelectFn`; select output also passes through `replaceData(prevResult?.data, …)`; **select throw is captured** → `#selectError`, then `error = selectError; data = selectResult; errorUpdatedAt = Date.now(); status='error'; isPlaceholderData=false`. Stored select error is cleared when data becomes undefined.
- Derived flags: `isLoading = isPending && isFetching`; `isRefetching = isFetching && !isPending`; `isLoadingError = isError && !hasData`; `isRefetchError = isError && hasData`; `isFetchedAfterMount` compares `dataUpdateCount/errorUpdateCount` against the query-initial state captured at observer attach; `isStale = enabled !== false && query.isStaleByTime(staleTime)`; `isEnabled`; `failureCount/failureReason` come from `fetchFailureCount/fetchFailureReason`.
- Full result shape (§9.5).

Notification filtering (`shouldNotifyListeners`):
1. no previous result → notify;
2. `notifyOnChangeProps === 'all'` → notify;
3. `!notifyOnChangeProps && trackedProps.size === 0` → notify (nothing tracked);
4. `throwOnError && currentResult.error !== prev.error` → notify;
5. otherwise notify iff any key in `notifyOnChangeProps ?? trackedProps` changed (reference compare per key).

Mount/refetch predicates (module-private, exact):
- `shouldLoadOnMount`: `enabled !== false && data === undefined && !(status === 'error' && retryOnMount === false)`.
- `shouldFetchOnMount`: `shouldLoadOnMount || (data !== undefined && shouldFetchOn(options.refetchOnMount))`.
- `shouldFetchOn(field)`: `enabled !== false && staleTime !== 'static' && (value === 'always' || (value !== false && isStale(query, options)))`.
- `shouldFetchOptionally`: `(query !== prevQuery || prevOptions.enabled === false) && !(options.suspense && status === 'error') && isStale(...)`.

Timers (all via `timeoutManager`; never scheduled on server, when `enabled === false`, or invalid timeout):
- stale timeout: scheduled only when result is NOT stale; fires `updateResult()` at `timeUntilStale(dataUpdatedAt, staleTime) + 1` ms (**+1ms mitigation** for early fire).
- refetch interval: `setInterval(computedInterval)`; `computedInterval = resolveQueryValue(options.refetchInterval, query) ?? false`; skipped when `0`; the tick fetches only if `refetchIntervalInBackground || focusManager.isFocused()`.

### 8.2 `InfiniteQueryObserver` (`extends QueryObserver`)
- `setOptions` / `getOptimisticResult`: stamp `options._type = 'infinite'` then delegate.
- `fetchNextPage(options?: FetchNextPageOptions)` → `this.fetch({...options, meta: {fetchMore: {direction: 'forward'}}})`.
- `fetchPreviousPage(options?: FetchPreviousPageOptions)` → same with `direction: 'backward'`.
- `createResult`: spreads parent result, then:
  - `hasNextPage = hasNextPage(options, state.data)` (`getNextPageParam(lastPage) != null`),
  - `hasPreviousPage = hasPreviousPage(...)` (requires `getPreviousPageParam` set),
  - `isFetchNextPageError = isError && fetchDirection === 'forward'` (direction read from `state.fetchMeta?.fetchMore?.direction`),
  - `isFetchingNextPage = isFetching && direction === 'forward'`, same for previous/backward,
  - `isRefetchError = parent.isRefetchError && !isFetchNextPageError && !isFetchPreviousPageError`,
  - `isRefetching = parent.isRefetching && !isFetchingNextPage && !isFetchingPreviousPage`,
  - binds `fetchNextPage`/`fetchPreviousPage` into the result.
- Type-level result overrides: `subscribe`, `getCurrentResult`, `fetch` return `InfiniteQueryObserverResult`.

### 8.3 `MutationObserver` (`extends Subscribable<result => void>`)
| Member | Behavior |
|---|---|
| `options` | defaulted via `client.defaultMutationOptions`. |
| `setOptions(options)` | notify `observerOptionsUpdated` when options changed shallowly; **if both prev & next `mutationKey` defined and `hashKey` differs → `reset()`**; else if current mutation `pending` → `currentMutation.setOptions(this.options)` (live update of in-flight mutation). |
| `mutate(variables, options?: MutateOptions)` | store `#mutateOptions` (snapshot before callbacks); `currentMutation?.removeObserver(this)`; `currentMutation = mutationCache.build(client, this.options)` (new mutation per call!); `addObserver`; `execute(variables)`. Returns the mutation promise. |
| `reset()` | removeObserver + drop current mutation + recompute result (back to `idle`) + notify. Does NOT cancel an in-flight mutation. |
| `getCurrentResult()` | `{...state, isPending/isSuccess/isError/isIdle, mutate, reset}` — defaults to `getDefaultState()` when no mutation. |
| `onMutationUpdate(action)` | recompute + `#notify(action)`. |
| `onSubscribe`/`onUnsubscribe` | attach/detach to `#currentMutation` on first/last listener. |
| `#notify(action)` | per-call `MutateOptions` callbacks (onSuccess+onSettled / onError+onSettled, each try/caught) run **only if `hasListeners()`**, then listeners, inside `notifyManager.batch`. |

### 8.4 `QueriesObserver` (`extends Subscribable<results => void>`)
- `QueriesObserverOptions = { combine?: (results) => TCombinedResult }`.
- `setQueries(queries, options?)`: dev warning on duplicate hashes; batch; `#findMatchingObservers` matches by `queryHash` (reuse existing `QueryObserver`s; map-shift handles duplicates); set options on each match; structural change (length/index) vs result change (shallow per index); destroy removed observers / subscribe new ones; `#notify()`.
- `getCurrentResult()` → array; `getQueries()` → `Query[]`; `getObservers()` → `QueryObserver[]`.
- `getOptimisticResult(queries, combine)` → `[rawResult, combineResult, trackResult]` tuple (adapters call this per render).
- `combine` memoization: recompute when `#result !== #lastResult`, query hashes changed, or combine fn changed; result memoized through `replaceEqualDeep` (`#combinedResult`).
- `#shouldSkipCombine`: no combine fn, or any observer with `suspense` whose result has no data.
- Property tracking is synchronized across all observers (accessing a prop on one tracks it on all — issue #7000).
- `destroy()` clears listeners + destroys all inner observers.

---

## 9. Options reference (exact names + defaults)

### 9.1 `QueryOptions` (query-level)
| Option (camelCase / snake_case) | Type | Default | Notes |
|---|---|---|---|
| `retry` / `retry` | `boolean \| number \| ((failureCount, error) => boolean)` | resolved in retryer: **client 3, server 0**; `fetchQuery`/`query` force `false` when undefined; mutation path `?? 0` | |
| `retryDelay` / `retry_delay` | `number \| ((failureCount, error) => number)` | `failureCount => Math.min(1000 * 2 ** failureCount, 30000)` | **no jitter**; called with pre-increment failureCount (first retry = 1000 ms) |
| `networkMode` / `network_mode` | `'online' \| 'always' \| 'offlineFirst'` | `'online'` (implicit); `'offlineFirst'` when `persister` set (dependent default) | |
| `gcTime` / `gc_time` | `number` | `5 * 60 * 1000`; `Infinity` on server | Removable keeps `Math.max(existing, new)` — **gcTime never shrinks on an instance**; `Infinity` disables GC; max ~24.8 days (32-bit setTimeout) |
| `queryFn` / `query_fn` | `QueryFunction \| SkipToken` | required (else `Missing queryFn` rejection) | `skipToken` → `enabled = false` via defaultQueryOptions; cannot be refetched |
| `persister` / `persister` | `(queryFn, context, query) => T \| Promise<T>` | undefined | wraps fetchFn |
| `queryHash` / `query_hash` | `string` | computed `hashKey(queryKey)` | cache key; provided hash is trusted |
| `queryKey` / `query_key` | `TQueryKey` (array) | required for observers | hashed w/ sorted object keys |
| `queryKeyHashFn` / `query_key_hash_fn` | `(queryKey) => string` | `hashKey` | |
| `initialData` / `initial_data` | `TData \| (() => TData \| undefined)` | undefined | fn called once at query creation; persisted to cache; counts as `success` |
| `initialDataUpdatedAt` / `initial_data_updated_at` | `number \| (() => number \| undefined)` | undefined | `?? Date.now()` when initialData present |
| `behavior` | `QueryBehavior { onFetch(context, query) }` | undefined | infinite behavior injected automatically for `_type: 'infinite'` |
| `structuralSharing` / `structural_sharing` | `boolean \| ((oldData, newData) => unknown)` | `true` | `false` → raw replace; fn → called with (prev, next) |
| `meta` | `QueryMeta (Record<string, unknown>)` | undefined | available in `queryFn` context, callbacks, filters |
| `maxPages` / `max_pages` | `number` | undefined (0 = unlimited) | infinite page trimming |
| `_defaulted` | `boolean` (internal) | | set by `defaultQueryOptions`; short-circuits re-defaulting |
| `_type` | `'infinite'` (internal) | | set by infinite APIs/observers |

### 9.2 `QueryObserverOptions` additions (observer-level)
| Option | Type | Default |
|---|---|---|
| `enabled` | `boolean \| (query) => boolean` | `true` |
| `staleTime` / `stale_time` | `number \| 'static' \| (query) => StaleTime` | `0` |
| `refetchInterval` / `refetch_interval` | `number \| false \| (query) => number \| false \| undefined` | `false` |
| `refetchIntervalInBackground` | `boolean` | `false` |
| `refetchOnWindowFocus` | `boolean \| 'always' \| (query) => boolean \| 'always'` | `true` |
| `refetchOnReconnect` | same | `true` (dependent default `networkMode !== 'always'`) |
| `refetchOnMount` | same | `true` |
| `retryOnMount` / `retry_on_mount` | `boolean \| (query) => boolean` | `true` |
| `notifyOnChangeProps` | `Array<keyof result> \| 'all' \| (() => ...)` | undefined (track by usage) |
| `throwOnError` | `boolean \| (error, query) => boolean` | `false`; dependent default `!!suspense` |
| `select` | `(data: TQueryData) => TData` | undefined (observer-only transform; cache untouched; memoized) |
| `suspense` | `boolean` | `false` (excluded from `DefaultOptions.queries`) |
| `placeholderData` | `TQueryData \| (previousData, previousQuery) => TQueryData \| undefined` | undefined; helper `keepPreviousData(prev) => prev` |
| `_optimisticResults` | `'optimistic' \| 'isRestoring'` (internal) | |

### 9.3 `MutationOptions`
| Option | Type | Default |
|---|---|---|
| `mutationFn` / `mutation_fn` | `(variables, ctx) => Promise<TData>` | required (rejects `'No mutationFn found'`) |
| `mutationKey` / `mutation_key` | `MutationKey` | undefined |
| `onMutate` | `(variables, ctx) => Promise<TOnMutateResult> \| TOnMutateResult` | — awaited before mutationFn |
| `onSuccess` | `(data, variables, onMutateResult, ctx)` | awaited before onSettled |
| `onError` | `(error, variables, onMutateResult\|undefined, ctx)` | awaited |
| `onSettled` | `(data\|undefined, error\|null, variables, onMutateResult\|undefined, ctx)` | awaited |
| `retry` | `RetryValue` | **0** |
| `retryDelay` | `RetryDelayValue` | same exponential default |
| `networkMode` | `NetworkMode` | `'online'` |
| `gcTime` | `number` | 5 min / Infinity |
| `meta` | `MutationMeta` | undefined |
| `scope` | `MutationScope { id: string }` | undefined — same id runs serially |
| `_defaulted` | internal | |

`MutationObserverOptions` adds `throwOnError?: boolean | ((error) => boolean)` (default false).
`MutateOptions` (per-call): `onSuccess/onError/onSettled` (sync-only, void), run AFTER the option callbacks; only fire when the observer has listeners.

### 9.4 Context objects
- `QueryFunctionContext`: `{ client, queryKey, signal: AbortSignal, meta, pageParam?, direction? (deprecated) }`; infinite variant makes `pageParam: TPageParam` and `direction: FetchDirection` present.
- `MutationFunctionContext`: `{ client, meta, mutationKey? }`.
- `QueryPersister` receives `(queryFn, context, query)`.

### 9.5 Result shapes
- `QueryObserverBaseResult`: `data, dataUpdatedAt, error, errorUpdatedAt, failureCount, failureReason, errorUpdateCount, isError, isFetched, isFetchedAfterMount, isFetching, isLoading, isPending, isLoadingError, isInitialLoading (deprecated), isPaused, isPlaceholderData, isRefetchError, isRefetching, isStale, isSuccess, isEnabled, refetch(options?) => Promise<result>, status, fetchStatus`.
- Variants: `Pending / Loading / LoadingError / RefetchError / Success / Placeholder` (+ `DefinedQueryObserverResult` union) — all with literal-typed flags.
- Infinite adds: `fetchNextPage, fetchPreviousPage, hasNextPage, hasPreviousPage, isFetchNextPageError, isFetchingNextPage, isFetchPreviousPageError, isFetchingPreviousPage`.
- Mutation adds: `variables, isIdle, mutate, reset` (+ variants Idle/Loading/Error/Success).

### 9.6 Small option types
- `CancelOptions { revert?: boolean; silent?: boolean }`
- `SetDataOptions { updatedAt?: number }`
- `ResultOptions { throwOnError?: boolean }` (default false)
- `RefetchOptions extends ResultOptions { cancelRefetch?: boolean }` (default **true**)
- `InvalidateOptions = ResetOptions = RefetchOptions`
- `FetchNextPageOptions` / `FetchPreviousPageOptions` = `ResultOptions & { cancelRefetch?: boolean }` (default true)
- `StaleTime = number | 'static'`
- `FetchDirection = 'forward' | 'backward'`
- `NotifyEventType = 'added' | 'removed' | 'updated' | 'observerAdded' | 'observerRemoved' | 'observerResultsUpdated' | 'observerOptionsUpdated'`

---

## 10. Filters, statuses, hashing, structural sharing

### 10.1 `QueryFilters`
```ts
{ type?: 'all' | 'active' | 'inactive'   // default 'all' — active = has an enabled observer
  exact?: boolean                         // false → partialMatchKey; true → hash compare (uses queryKeyHashFn via query.options)
  predicate?: (query) => boolean
  queryKey?: TQueryKey | TuplePrefixes<TQueryKey>   // any array prefix allowed
  stale?: boolean                         // vs query.isStale()
  fetchStatus?: FetchStatus }
```
`matchQuery` order: queryKey → type → stale → fetchStatus → predicate.
`InvalidateQueryFilters` adds `refetchType?: 'active' | 'inactive' | 'all' | 'none'` (default `'active'`).

### 10.2 `MutationFilters`
```ts
{ exact?: boolean; predicate?: (mutation) => boolean;
  mutationKey?: TuplePrefixes<MutationKey>; status?: MutationStatus }
```
`matchMutation`: **a `mutationKey` filter never matches a mutation without its own key**; exact → `hashKey` equality; then status, then predicate.

### 10.3 Statuses
- `QueryStatus = 'pending' | 'error' | 'success'`
- `FetchStatus = 'fetching' | 'paused' | 'idle'`
- `MutationStatus = 'idle' | 'pending' | 'success' | 'error'`

### 10.4 Hashing (`hashKey`)
`JSON.stringify(key, replacer)` where the replacer **sorts plain-object keys alphabetically**. So `['todos', {page:1, filter:'done'}]` → `["todos",{"filter":"done","page":1}]`. Arrays keep order. Must be reproduced bit-exactly for devtools/SSR interop (rs-query `key.rs`).

`partialMatchKey(a, b)`: true if `a === b`; type mismatch → false; arrays → `b.length <= a.length` and element-wise recursive match; plain objects → every key of `b` must match `a` recursively; else false.

### 10.5 Structural sharing (`replaceEqualDeep(a, b, depth = 0)`)
1. `a === b` → return `a`.
2. `depth > 500` → return `b` (recursion cap).
3. Only recurses when both sides are plain arrays (`Array.isArray && length === Object.keys(value).length`) or both plain objects (`isPlainObject`: `[object Object]` tag + prototype checks); otherwise returns `b`.
4. Per item: identical → copy ref (counts equal); non-object mismatch → copy `b`; else recurse `replaceEqualDeep(aItem, bItem, depth+1)`; recursion result `=== aItem` counts equal.
5. Return `a` iff sizes equal AND every item counted equal, else the new copy.
`replaceData(prevData, data, options)` is the wrapper used by `Query.setData` and observer select/placeholder: custom fn → call it; `structuralSharing !== false` → `replaceEqualDeep` (dev: catches & logs `Structural sharing requires data to be JSON serializable…` then rethrows); else raw data.

---

## 11. Retryer & backoff (exact)

```ts
createRetryer(config: {
  fn: () => TData | Promise<TData>,
  initialPromise?: Promise<TData>,      // used only for the first run (failureCount === 0)
  onCancel?: (error) => void,
  onFail?: (failureCount, error) => void,
  onPause?: () => void,
  onContinue?: () => void,
  retry?: RetryValue,                    // boolean | number | (failureCount, error) => boolean
  retryDelay?: RetryDelayValue,          // number | (failureCount, error) => number
  networkMode: NetworkMode | undefined,
  canRun: () => boolean,                 // mutation scope gate; queries pass () => true
}): Retryer
```

`Retryer` interface: `promise`, `cancel(cancelOptions?)`, `continue() => Promise`, `cancelRetry()`, `continueRetry()`, `canStart()`, `start()`, `status() => 'pending'|'resolved'|'rejected'`.

**Exact default backoff formula — no jitter:**
```
delay(failureCount) = Math.min(1000 * 2 ** failureCount, 30000)
```
- Computed with the **pre-increment** count inside the catch: 1st failure → 1000 ms, 2nd → 2000, 3rd → 4000 … capped 30 s.
- Sequence per attempt: fail → compute delay → `failureCount++` → `onFail(failureCount, error)` → `sleep(delay)` → if `!canContinue()` → `pause(canContinue)` → if `isRetryCancelled` reject else `run()` again.
- `shouldRetry = retry === true || (typeof retry === 'number' && failureCount < retry) || (typeof retry === 'function' && retry(failureCount, error))` — evaluated before the increment (so `retry: 3` = initial try + 3 retries).
- Default `retry`: `config.retry ?? (isServer() ? 0 : 3)`.

**Network/focus gating:**
- `canFetch(networkMode) = (networkMode ?? 'online') === 'online' ? onlineManager.isOnline() : true` → `'always'`/`'offlineFirst'` may start offline; `'online'` may not.
- `canStart() = canFetch(networkMode) && config.canRun()` (mutation scope check).
- `canContinue() = focusManager.isFocused() && (networkMode === 'always' || onlineManager.isOnline()) && config.canRun()` — note **focus** matters for continuing retries but NOT for starting (queued-behind-scope explicitly does not depend on focus).
- `start()`: `canStart() ? run() : pause(canStart).then(run)`.
- `pause(canResume)` stores `continueFn`; `continue()` calls it; the wait resolves only when resolved/rejected or `canResume()` is true; `onContinue` dispatch after wake.
- `cancel(cancelOptions)`: reject with `CancelledError(options)` + `onCancel(error)` (only while pending). `CancelledError extends Error { revert?: boolean; silent?: boolean }`; `isCancelledError` deprecated (use instanceof).
- Promise is created up-front with `.catch(noop)` to avoid unhandled rejections; `resolve`/`reject` are idempotent and call `continueFn` first.

---

## 12. Managers & infra

### 12.1 `Subscribable<TListener>`
`subscribe(listener) => unsubscribe` (listener stored in a `Set`; bound method); `hasListeners()`; protected `onSubscribe()`/`onUnsubscribe()` hooks (first add / last remove).

### 12.2 `Removable` (GC base for Query & Mutation)
`gcTime` field; `scheduleGc()` (clear old timer; schedule `optionalRemove` after `gcTime` via `timeoutManager` unless `Infinity`/invalid); `updateGcTime(new?)` keeps `Math.max(this.gcTime || 0, new ?? (isServer ? Infinity : 5*60*1000))`; `clearGcTimeout()`; `destroy()` clears the timer (Query overrides to also silently cancel).

### 12.3 `focusManager` (`FocusManager extends Subscribable<(focused: boolean) => void>`)
- `setFocused(focused?: boolean)` — manual override; `undefined` → fall back to default check; notifies only on change.
- `isFocused()` — manual value, else `globalThis.document?.visibilityState !== 'hidden'`.
- `onFocus()` — notify all listeners with current state.
- `setEventListener(setup)` — setup receives `handleFocus(bool?)` and returns cleanup; replaces previous listener.
- Default listener: `window.addEventListener('visibilitychange', …)`, attached on first subscribe, removed when last unsubscribes.

### 12.4 `onlineManager` (`OnlineManager extends Subscribable<(online: boolean) => void>`)
- `setOnline(online: boolean)` — set + notify on change. `isOnline()` — starts `true`.
- `setEventListener(setup)` — setup receives `setOnline(bool)`; default listeners: `window` `online`/`offline`.

### 12.5 `notifyManager` (singleton via `createNotifyManager()`)
- `batch<T>(cb) => T` — nested via `transactions` counter; flush on outermost exit (return value passes through).
- `batchCalls(cb)` — wraps fn so every call is `schedule(() => cb(...args))`.
- `schedule(cb)` — inside a batch → queued; else scheduled immediately via the scheduler.
- `setNotifyFunction(fn)` (wrap each callback), `setBatchNotifyFunction(fn)` (wrap a whole flushed batch — adapters plug their own batching), `setScheduler(fn)` (default `defaultScheduler = systemSetTimeoutZero` = `setTimeout(cb, 0)`). **v5 has no `scheduleMicrotask`.**
- Flush: swap queue, run `batchNotifyFn(() => queue.forEach(cb => notifyFn(cb)))` via scheduler.

### 12.6 `timeoutManager` (`TimeoutManager` singleton)
`setTimeout(cb, delay): ManagedTimerId`, `clearTimeout(id)`, `setInterval(cb, delay)`, `clearInterval(id)`, `setTimeoutProvider(provider)`. `TimeoutProvider` = the four functions; `defaultTimeoutProvider` wraps globals (via wrapper fns so tests can spy). `ManagedTimerId = number | { [Symbol.toPrimitive]: () => number }`. `systemSetTimeoutZero(cb)` exported for notifyManager. All GC, stale, refetch-interval, retry-sleep timers go through it (rs-query: one `tokio`/`gpui` timer abstraction).

### 12.7 `environmentManager`
`isServer(): boolean` + `setIsServer(fn)`; default detection: `typeof window === 'undefined' || 'Deno' in globalThis` (drives retry default 3→0, gcTime Infinity, disabling stale/refetch timers).

### 12.8 `experimental_streamedQuery`
`streamedQuery({ streamFn, refetchMode = 'reset', reducer = (items, chunk) => [...items, chunk], initialValue = [] })` → `queryFn`. Semantics: `pending` until first chunk, `success` after it, `fetchStatus: 'fetching'` until stream end. On refetch with `refetchMode: 'reset'` → `setState({...resetState, fetchStatus:'fetching'})` before streaming; `'append'` writes each chunk via `setQueryData`; `'replace'` accumulates locally and writes once at the end. Cancellation via consume-aware signal breaks the loop. Returns final cache data (or initialValue).

---

## 13. Hydration / dehydration (exact logic)

```ts
interface DehydratedState {
  mutations: Array<{ mutationKey?, state: MutationState, meta?, scope? }>
  queries:   Array<{ queryHash, queryKey, state: QueryState, dehydratedAt: number,
                     promise?: Promise<unknown>, meta?, queryType?: 'infinite' }>
}
```

**dehydrate(client, options = {})**:
- `options.shouldDehydrateMutation ?? client.getDefaultOptions().dehydrate?.shouldDehydrateMutation ?? defaultShouldDehydrateMutation` — default keeps **only `state.isPaused === true`** mutations. Dehydrated mutation = `{mutationKey, state, scope?, meta?}`.
- `options.shouldDehydrateQuery ?? … ?? defaultShouldDehydrateQuery` — default keeps **only `status === 'success'`** queries.
- `dehydrateQuery`: `{ dehydratedAt: Date.now(), state: {...query.state, data: serializeData ? serializeData(data) : data (only if data !== undefined)}, queryKey, queryHash, promise? (only when status === 'pending'), meta?, queryType? }`.
- Pending promise handling (`dehydratePromise`): `query.promise.then(serializeData).catch(err => shouldRedactErrors?.(err) === false ? reject(err) : reject(new Error('redacted')))`; the derived promise gets `.catch(noop)` to avoid unhandled rejections. `shouldRedactErrors` default (undefined) → **redact**. No maxAge is applied by dehydrate itself.
- Timestamp handling: `dehydratedAt` is the wall clock at dehydration; it is compared against the target cache's `dataUpdatedAt` during hydrate, and is used as `dataUpdatedAt` when a dehydrated-pending query has resolved data by hydrate time.

**hydrate(client, dehydratedState: Partial<DehydratedState>, options?)** — mutations **first**, then queries:
- Mutations: `mutationCache.build(client, {...defaultOptions.hydrate?.mutations, ...options?.defaultOptions?.mutations, ...mutationOptions}, state)` — restores raw state including `status: 'pending'`, `isPaused: true` (so `resumePausedMutations` can pick them up).
- Queries: `deserializeData = options?.defaultOptions?.deserializeData ?? defaultOptions.hydrate?.deserializeData`; `syncData = tryResolveSync(promise)` (promise already resolved synchronously); `rawData = state.data === undefined ? syncData?.data : state.data`; `data = deserializeData(rawData)`.
- Existing query (by `queryHash`): hydrate only when `state.dataUpdatedAt > existing.state.dataUpdatedAt || (syncData && dehydratedAt > existing.dataUpdatedAt)`. Then `setState({...state minus fetchStatus, data, ...})`; a dehydrated-`pending` state with data present is upgraded to `status: 'success'`, `dataUpdatedAt: dehydratedAt`, and `fetchStatus: 'idle'` unless the existing query is currently fetching. Existing fetchStatus is otherwise preserved (omitted from the merge).
- New query: `queryCache.build(client, {...hydrate default queries, queryKey, queryHash, meta, _type: queryType}, {...state, data, fetchStatus: 'idle', status: pending&&data!==undefined ? 'success' : state.status, dataUpdatedAt: dehydratedAt when upgraded})` — **fetchStatus forced to `'idle'` so a hydrated query is never stuck "fetching"**.
- In-flight promise: if `promise && !syncData && !existingQueryIsPending && !existingQueryIsFetching && dehydratedAt > query.state.dataUpdatedAt` → `query.fetch(undefined, { initialPromise: Promise.resolve(promise).then(deserializeData) }).catch(noop)` — this does not call `queryFn`; the retryer adopts the promise (and `ensureQueryFn` returns the initialPromise for retries when there's no queryFn).
- Hydration without a `queryFn` is fine: the created query simply has no fn; `fetch` later adopts the first observer's options that has one.
- Note: `structuralSharing` is NOT applied on hydrate (raw setState / build state); only `deserializeData` transforms.

---

## 14. Persister (adjacent package `@tanstack/query-persist-client-core`)

### 14.1 `experimental_createQueryPersister(options)` → `{ persisterFn, persistQuery, persistQueryByKey, retrieveQuery, persisterGc, restoreQueries, removeQueries }`
Options (`StoragePersisterOptions`):
| Option | Default | Notes |
|---|---|---|
| `storage: AsyncStorage \| undefined \| null` | required conceptually | `{ getItem, setItem, removeItem, entries? }`; values `string` by default |
| `serialize` / `deserialize` | `JSON.stringify` / `JSON.parse` | of a `PersistedQuery` |
| `buster` | `''` | mismatched buster → discard |
| `maxAge` | `24 * 60 * 60 * 1000` | entry expired when `Date.now() - state.dataUpdatedAt > maxAge` |
| `prefix` | `'tanstack-query'` (`PERSISTER_KEY_PREFIX`) | storage key = `` `${prefix}-${queryHash}` `` |
| `refetchOnRestore` | `true` | `'always'` → always refetch after restore; `true` → only if stale; `false` → never |
| `filters` | undefined | `QueryFilters` gating which queries persist |

`PersistedQuery = { buster, queryHash, queryKey, state: QueryState }`. Validity check requires string `queryHash`, array `queryKey`, plain-object `state` with finite `dataUpdatedAt`; invalid/old/busted entries are **removed from storage**.

- `persisterFn(queryFn, ctx, query)`: if matches filters and `query.state.data === undefined` → `retrieveQuery(query.queryHash, afterRestore)`; restored data resolves the fetch (no `queryFn` call). `afterRestore` (scheduled via `notifyManager.schedule`) fixes `dataUpdatedAt`/`errorUpdatedAt` on the query (the first resolution stamped `Date.now()`) and triggers `query.fetch()` per `refetchOnRestore`. Otherwise runs `queryFn` and schedules `persistQuery(query)` (deferred so state is final).
- `persistQuery(query)` / `persistQueryByKey(queryKey, queryClient)`: serialize `{state, queryKey, queryHash, buster}` under `${prefix}-${queryHash}`.
- `persisterGc()`: needs `storage.entries()`; removes expired/busted/invalid entries with the prefix.
- `restoreQueries(queryClient, filters?: {queryKey?, exact?})`: needs `entries()`; for each valid entry matching filters → `queryClient.setQueryData(queryKey, state.data, {updatedAt: state.dataUpdatedAt})` (preserves original freshness).
- `removeQueries(filters?)`: needs `entries()`; removes matching keys from storage.

### 14.2 Whole-client persistence (`persist.ts`)
- `Persister { persistClient, restoreClient, removeClient }`; `PersistedClient { timestamp, buster, clientState: DehydratedState }`.
- `persistQueryClientRestore({queryClient, persister, maxAge = 24h, buster = '', hydrateOptions})`: restore → expired (`Date.now() - timestamp > maxAge`) or busted or missing timestamp → `removeClient()`; else `hydrate(...)`. On error: log (dev), `removeClient()`, rethrow.
- `persistQueryClientSave({queryClient, persister, buster = '', dehydrateOptions})`: `persistClient({buster, timestamp: Date.now(), clientState: dehydrate(client, dehydrateOptions)})`.
- `persistQueryClientSubscribe(props)`: subscribes both caches; saves only on `'added' | 'removed' | 'updated'` events (not observer events).
- `persistQueryClient(props)` → `[unsubscribe, restorePromise]`.
- `retryStrategies.removeOldestQuery`: `PersistRetryer` that drops the query with the oldest `dataUpdatedAt` on save failure.

---

## 15. Subtle behaviors — porting notes (the ones that bite)

1. **Retry math**: delay computed BEFORE `failureCount++`; formula `min(1000 * 2^n, 30000)`; no jitter; retries pause on unfocus (except `networkMode: 'always'` keeps the network check off but focus still gates continuation); retry decision uses pre-increment count.
2. **Fetch dedup**: concurrent `fetch` on the same query returns the same retryer promise; `cancelRefetch: true` (default everywhere in client methods) only silently cancels the running fetch **when data already exists**; rejected retryer always restarts.
3. **`AbortSignal` propagation**: fresh `AbortController` per fetch; signal is a lazy property — `abortSignalConsumed` is only set if `queryFn` reads it; on last observer unsubscribe: consumed or paused-pending → `cancel({revert: true})`, otherwise just `cancelRetry()` (let the fetch finish to cache it). Retryer `onCancel` aborts the controller.
4. **networkMode pause/resume**: fetch state starts `'paused'` when `canFetch` false; `Query.onFocus`/`onOnline` call `retryer.continue()`; mutation pause → `isPaused: true` state (dehydratable); `queryClient.resumePausedMutations()` is a no-op offline.
5. **Mutation scopes**: `canRun` gate inside retryer; `runNext` after settle; `continue()` also revives dehydrated pending mutations (`execute(state.variables!)`).
6. **GC interplay**: `updateGcTime` is monotonic (max); `scheduleGc` after fetch; observers pause GC; mutations defer GC while pending; gc only removes idle, observer-less queries.
7. **invalidate vs remove vs reset vs cancel**: invalidate = flag only (+ refetch via invalidateQueries); reset = destroy + initial state + refetch actives; remove = destroy + drop from cache (removes hash → observers will rebuild); cancel = abort in-flight (default `revert: true` via cancelQueries → restores pre-fetch state, resolves with old data when it exists, rejects CancelledError when it doesn't).
8. **Observer notify pipeline**: `notifyManager.batch` everywhere; `updateResult` skips work on shallow-equal results; tracked-props default filtering; `throwOnError` forces notification on error change; `observerResultsUpdated` fired after listeners.
9. **Stale timer +1ms** and server/focus guards on timers; refetch interval skips when `0` or disabled.
10. **Infinite queries**: direction carried via `fetchMeta` (also drives `isFetchingNextPage` etc.); refetch-all loop restarts from `oldPageParams[0] ?? initialPageParam` and re-fetches every cached page (or `pages` count) forward; `maxPages` trims via `addToEnd/addToStart` (only one item dropped per add); `param == null` stops pagination; `fetchNextPage` default `cancelRefetch: true` (repeated calls restart the fetch and ignore earlier results), `false` reuses the running fetch.
11. **`select` pitfalls**: memoized per (data-ref, fn-ref); runs on placeholderData too; select throw becomes result error with `errorUpdatedAt = now`; `fetchQuery/query` also apply select to the resolved value.
12. **`_defaulted` marker** prevents double-defaulting when options objects flow between APIs (queryOptions factories etc.). Port as a flag on the Rust options struct.
13. **Dev-only invariants** (port as `debug_assert!`/logging): queryKey must be array; `data === undefined` from queryFn is an error; skipToken invoked; duplicate QueriesObserver hashes; structural sharing non-JSON error; dehydrated-pending rejection logged + redacted.
14. **Error state always sets `isInvalidated: true`** — background errors make cached data stale.
15. **Cache-level callbacks**: `QueryCacheConfig` callbacks are fire-and-forget; `MutationCacheConfig` callbacks are awaited and individually guarded on the error path.

---

## 16. PORTING CHECKLIST

Rust hints reference existing rs-query modules; "(new)" means a new module/file is needed.

### Core types & utils
- [ ] `QueryKey` + `hashKey` (sorted-key JSON stringify) — `key.rs`, verify object-key sorting parity.
- [ ] `partialMatchKey` (recursive prefix matcher) — `key.rs`.
- [ ] `QueryStatus`/`FetchStatus`/`MutationStatus`/`FetchDirection` enums — `state.rs`.
- [ ] `Updater<T>` (`value | fn(prev)`) + `functionalUpdate` — `options.rs` or (new) `updater.rs`.
- [ ] `replaceEqualDeep` (depth cap 500, plain arrays/objects only) — `sharing.rs`, add depth guard + `is_plain_object/is_plain_array` equivalents.
- [ ] `replaceData` wrapper honoring `structural_sharing: bool | fn` — `sharing.rs`.
- [ ] `shallowEqualObjects` for result diffing — `observer.rs`/`state.rs`.
- [ ] `timeUntilStale` / `isValidTimeout` / `resolveQueryValue` (option-as-fn resolution) — `options.rs`.
- [ ] `matchQuery` / `matchMutation` + `QueryFilters`/`MutationFilters` structs (`type/exact/predicate/query_key/stale/fetch_status`, `mutation_key/status`) — (new) `filters.rs`.
- [ ] `hashQueryKeyByOptions` honoring `query_key_hash_fn` — `key.rs`.
- [ ] `skipToken` sentinel (disable + `enabled=false` in defaulting) — `options.rs`.
- [ ] `keepPreviousData`, `addToEnd`/`addToStart` (maxPages trim) — `options.rs` / `infinite.rs`.
- [ ] `ensureQueryFn` (missing-fn rejection, initialPromise retry fallback) — `query.rs`.
- [ ] `shouldThrowError` (throwOnError fn resolution) — `observer.rs`.
- [ ] `addConsumeAwareSignal`-equivalent: lazy cancellation token consumption flag — `cancellation.rs`.
- [ ] `noop` equivalent (in Rust: `|_| {}` / `Ok(())` idioms) — n/a.

### Retryer
- [ ] `Retryer` struct: promise analog (e.g. `oneshot` + shared future), `cancel/cancel_retry/continue_retry/continue/can_start/start/status` — (new) `retryer.rs`.
- [ ] Default backoff `min(1000 * 2^failure_count, 30_000)` ms, computed pre-increment, no jitter — `retryer.rs`.
- [ ] `can_fetch(network_mode)` + `can_start`/`can_continue` gating incl. focus check on continue — `retryer.rs` + `focus_manager.rs`.
- [ ] `CancelledError { revert, silent }` error type + `is_cancelled_error` — `error.rs` (exists; align fields).
- [ ] `initialPromise` reuse on first run (hydrate hand-off) — `retryer.rs`/`executor.rs`.
- [ ] `sleep` via unified timer abstraction — `retryer.rs`.

### Managers
- [ ] `Subscribable` equivalent (listener set + unsubscribe) — (new) `subscribable.rs` or reuse GPUI events.
- [ ] `FocusManager` (`set_focused/is_focused/on_focus/set_event_listener`, ref-counted listeners) — `focus_manager.rs`, add `set_focused` + notify-on-change.
- [ ] `OnlineManager` (`set_online/is_online/set_event_listener`) — (new) `online_manager.rs` (start `true`).
- [ ] `NotifyManager`: `batch` (nested transactions), `batch_calls`, `schedule`, `set_notify_function/set_batch_notify_function/set_scheduler` — (new) `notify_manager.rs`; GPUI: batch ⇒ single `cx.notify()`.
- [ ] `TimeoutManager` abstraction (setTimeout/clearTimeout/setInterval/clearInterval + provider swap) — (new) `timeout_manager.rs`; used by GC/stale/interval/retry sleep.
- [ ] `EnvironmentManager` (`is_server()` override; gates retry=0, gc=∞, no timers) — (new) `environment.rs`.

### Removable / GC
- [ ] `Removable` base: `schedule_gc`, `update_gc_time` (Math.max semantics), `clear_gc_timeout`, `optional_remove` — `query.rs`/`mutation.rs`.

### QueryCache
- [ ] `QueryCache` storage keyed by `query_hash` + `build/add/remove/clear/get/get_all/find(exact default)/find_all/notify/on_focus/on_online` — (new) `query_cache.rs`.
- [ ] Cache events enum incl. `observerAdded/Removed/ResultsUpdated/OptionsUpdated` + `action` payload — `query_cache.rs`.
- [ ] `QueryCacheConfig { on_error, on_success, on_settled }` (fire-and-forget) — `query_cache.rs`.

### Query
- [ ] `Query` struct: `query_key/query_hash/options/state/observers` + getters `meta/query_type/promise/reset_state` — `query.rs`.
- [ ] State reducer with actions `failed/pause/continue/fetch/success/error/invalidate/set_state` (exact field effects incl. `is_invalidated=true` on error) — `state.rs`/`query.rs`.
- [ ] `set_options` (merge over per-query defaults, initialData backfill) — `query.rs`.
- [ ] `set_data` (structural sharing + manual flag) / `set_state` — `query.rs`.
- [ ] `cancel` / `destroy` (silent cancel) / `reset` (destroy + initial state) / `invalidate` — `query.rs`.
- [ ] `is_active/is_disabled/is_fetched/is_static/is_stale/is_stale_by_time` — `query.rs`.
- [ ] `on_focus`/`on_online` (refetch first willing observer w/ cancelRefetch:false + retryer.continue) — `query.rs`.
- [ ] `add_observer/remove_observer` (last-observer cancel semantics: consumed-signal or paused-pending ⇒ revert-cancel, else cancel-retry) — `query.rs`.
- [ ] `get_observers_count` — `query.rs`.
- [ ] `fetch()` full algorithm: dedup/return-existing-promise, silent-cancel on cancelRefetch w/ data, adopt observer queryFn when missing, fresh AbortController + lazy signal, behavior hook, `fetch` dispatch guard (idle or meta change), revert-state handling, undefined-data error, cache callbacks, retryer teardown + scheduleGc in finally — `query.rs` + `executor.rs`.
- [ ] `fetchState()` helper (fetching vs paused by networkMode, pending reset when no data) — `state.rs`.

### Mutation
- [ ] `Mutation` struct + `MutationState` (incl. `context`, `submitted_at`) — `mutation.rs`/`state.rs`.
- [ ] `set_options`, `add/remove_observer`, gc deferral while pending — `mutation.rs`.
- [ ] `execute(variables)` exact callback ordering (cache on_mutate → options on_mutate → retryer → cache/options onSuccess → cache/options onSettled; individually-caught error path) — `mutation.rs`.
- [ ] `continue()` (retryer resume, dehydrated-pending revival) — `mutation.rs`.
- [ ] `MutationCache`: Set-based store, `mutation_id` counter, scope queues, `build/add/remove/can_run/run_next/clear/get_all/find/find_all/notify/resume_paused_mutations` — (new) `mutation_cache.rs`.
- [ ] `MutationCacheConfig` awaited callbacks (on_mutate/on_success/on_error/on_settled) — `mutation_cache.rs`.
- [ ] Mutation scope `{ id }` serialization via `scope` map + `can_run` gate in retryer `can_run` — `mutation_cache.rs` + `retryer.rs`.

### QueryClient
- [ ] Constructor (inject caches + `default_options` incl. `hydrate`/`dehydrate` sections) — `client.rs`.
- [ ] `mount`/`unmount` ref-counted (focus/online subscriptions + resumePausedMutations then onFocus/onOnline) — `client.rs` + `focus_manager.rs`.
- [ ] `is_fetching` / `is_mutating` — `client.rs`.
- [ ] `get_query_data` / `set_query_data` (undefined-updater no-op; manual:true) / `get_queries_data` / `set_queries_data` — `client.rs`.
- [ ] `get_query_state` — `client.rs`.
- [ ] `fetch_query` (retry=false forcing) + `prefetch_query` + `ensure_query_data` (revalidate_if_stale) — `client.rs`.
- [ ] `query()` (new API: select applied to resolved value) — `client.rs`.
- [ ] Infinite variants: `infinite_query`/`fetch_infinite_query`/`prefetch_infinite_query`/`ensure_infinite_query_data` (`_type='infinite'`, `pages` option) — `client.rs` + `infinite_executor.rs`.
- [ ] `invalidate_queries` (refetchType precedence `refetchType ?? type ?? 'active'`, 'none' short-circuit) — `client.rs`.
- [ ] `refetch_queries` (skip disabled/static; paused ⇒ resolve immediately; cancelRefetch default true; throwOnError opt-in) — `client.rs`.
- [ ] `reset_queries` (reset + refetch active matched via predicate) — `client.rs`.
- [ ] `cancel_queries` (default revert:true; never rejects) — `client.rs`.
- [ ] `remove_queries` — `client.rs`.
- [ ] `resume_paused_mutations` (online-gated) — `client.rs`.
- [ ] `get/set_default_options`, `get/set_query_defaults`, `get/set_mutation_defaults` (registration-order merge via partial match) — `client.rs`.
- [ ] `default_query_options` (merge order, `_defaulted`, queryHash computation, dependent defaults: refetchOnReconnect/throwOnError/persister→offlineFirst/skipToken→enabled=false) — `client.rs`/`options.rs`.
- [ ] `default_mutation_options` — `client.rs`.
- [ ] `clear` — `client.rs`.

### Observers
- [ ] `QueryObserver`: constructor/defaulted options, `set_options` (enabled type check, query switch, observerOptionsUpdated, optional fetch, timer rescheduling) — `observer.rs`.
- [ ] `get_optimistic_result` (mutating cursor semantics) / `get_current_result` — `observer.rs`.
- [ ] `track_result`/`track_prop` (tracked-props notify filtering; GPUI: map to field-diff tracking) — `observer.rs`.
- [ ] `refetch` / `fetch_optimistic` / internal fetch w/ cancelRefetch default — `observer.rs`.
- [ ] `create_result` exact flag computation + placeholderData memo + select memo/error capture — `observer.rs` (partially exists; verify field-by-field).
- [ ] `update_result` notify rules ('all', untracked ⇒ notify, throwOnError ⇒ error-change notify) — `observer.rs`.
- [ ] Mount predicates `should_load_on_mount/should_fetch_on_mount/should_fetch_on/should_fetch_optionally` (incl. retryOnMount, 'always', static) — `observer.rs`.
- [ ] Stale timeout (+1 ms) & refetch interval (focus gate, `refetch_interval_in_background`) — `observer.rs` + `timeout_manager.rs`.
- [ ] `InfiniteQueryObserver`: `_type` stamping, `fetch_next_page`/`fetch_previous_page` via fetchMeta, `has_next/previous_page`, direction-derived flags — `infinite.rs`/`observer.rs`.
- [ ] `infiniteQueryBehavior(pages)`: single-page fetchNext/Previous vs refetch-all loop (reuse `oldPageParams[0]`, break on null param, maxPages trim), persister wrap, consume-aware cancel — `infinite_executor.rs`/`infinite.rs`.
- [ ] `has_next_page`/`has_previous_page` helpers (null checks incl. `getPreviousPageParam` presence) — `infinite.rs`.
- [ ] `MutationObserver`: `set_options` (mutationKey-change ⇒ reset; live update of pending mutation), `mutate` (new Mutation per call + per-call MutateOptions w/ snapshot), `reset`, `on_mutation_update`, notify gating on listeners — (new) `mutation_observer.rs`.
- [ ] `QueriesObserver`: `set_queries` (hash matching/reuse, dup warning), `get_optimistic_result` tuple, `combine` memoization via replaceEqualDeep, skip-combine on suspense-pending — (new) `queries_observer.rs`.

### Hydration
- [ ] `dehydrate`/`dehydrate_query`/`dehydrate_promise` (dehydratedAt, redaction default, serializeData) — `hydration.rs`.
- [ ] `default_should_dehydrate_mutation` (isPaused) / `default_should_dehydrate_query` (success) + `DefaultOptions.dehydrate` fallbacks — `hydration.rs`.
- [ ] `hydrate`: mutations-first ordering, sync-resolved promise fast path, newer-data guard (`dataUpdatedAt`/`dehydratedAt`), fetchStatus strip/idle, pending→success upgrade, initialPromise re-fetch — `hydration.rs`.
- [ ] `DehydratedState`/`DehydratedQuery`/`DehydratedMutation` serde shapes (match field names exactly for interop) — `hydration.rs`.
- [ ] `HydrateOptions { deserialize_data, queries, mutations }` — `hydration.rs`.

### Persistence
- [ ] `experimental_createQueryPersister` port: `AsyncStorage` trait, options (`storage/buster/max_age=24h/prefix='tanstack-query'/serialize/deserialize/refetch_on_restore/filters`), returned API `{persister_fn, persist_query, persist_query_by_key, retrieve_query, persister_gc, restore_queries, remove_queries}` — (new) `persister.rs`.
- [ ] `PersistedQuery { buster, query_hash, query_key, state }` serde + validity/expiry checks — `persister.rs`.
- [ ] persisterFn flow: restore-only-when-no-data, post-restore timestamp fixup + refetchOnRestore, deferred persist via notify schedule — `persister.rs`.
- [ ] Whole-client persistence: `Persister` trait, `persist_query_client_restore/save/subscribe/subscribe` (cache-event-only saves), `PersistedClient {timestamp, buster, client_state}` — (new) `persist_client.rs`.
- [ ] `remove_oldest_query` retry strategy — `persist_client.rs`.

### Misc
- [ ] `streamedQuery` (`refetch_mode: reset|append|replace`, reducer/initialValue, cancel-aware loop) — (new) `streamed_query.rs`.
- [ ] `QueryCache`/`Query` dev invariants as `debug_assert!`s (array key, undefined data, skipToken misuse) — `query.rs`.
- [ ] Query dedup test: two concurrent fetches share one executor — `executor.rs`.
- [ ] Cache event enum parity for devtools (`added/removed/updated/observer*`) — `devtools.rs`.
