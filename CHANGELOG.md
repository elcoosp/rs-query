# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/) and this
project adheres to [Semantic Versioning](https://semver.org/). Generated with
[git-cliff](https://git-cliff.org).
## [0.2.1] - 2026-11-14

### Bug Fixes
- allow dead_code for test utilities to pass strict CI by @elcoosp ([dc91883](https://github.com/elcoosp/rs-query/commit/dc918839561fe841810caff8316a893a5fe64599))
- update quoin-testkit lockfile to latest commit by @elcoosp ([ebeee5a](https://github.com/elcoosp/rs-query/commit/ebeee5a50262bfd96aff7e7fbf02bf0575bf1d16))
- enable tokio feature on quoin-testkit for TokioExecutor by @elcoosp ([62f2980](https://github.com/elcoosp/rs-query/commit/62f2980c96dd33a6740486579909fb2e9a4c62cf))
- use TokioExecutor::global instead of trait by @elcoosp ([129d07c](https://github.com/elcoosp/rs-query/commit/129d07cffaedf0a04f08e5db7302b3a54ce4855a))
- update quoin-testkit for LocalExecutor fix by @elcoosp ([7a9ec36](https://github.com/elcoosp/rs-query/commit/7a9ec364acc5c3ded3ddd91be9fce66ead6275eb))
- update quoin-testkit for E0753 fix by @elcoosp ([69a0c59](https://github.com/elcoosp/rs-query/commit/69a0c590597cd6c4f07aeadfd0786024ae82812d))
- update quoin-testkit by @elcoosp ([879065b](https://github.com/elcoosp/rs-query/commit/879065ba655af93dcb56d638e3ef9794303c3a19))
- update quoin-testkit by @elcoosp ([d9567b9](https://github.com/elcoosp/rs-query/commit/d9567b938a89e05765ae4824c0e04df0fc45887d))
- update quoin-testkit by @elcoosp ([bec36eb](https://github.com/elcoosp/rs-query/commit/bec36eb12683fdb23f5ccd578d5d4c382ec9b51f))
- update quoin-testkit for public LocalExecutor by @elcoosp ([b2f9568](https://github.com/elcoosp/rs-query/commit/b2f956820b88361d76725d744b56071c032b9947))
- update quoin-testkit to fix private LocalExecutor by @elcoosp ([d9eeca2](https://github.com/elcoosp/rs-query/commit/d9eeca22fce91bdbf5b943b2b2daf2430789daae))
- mv LocalExecutor by @elcoosp ([1ed90f2](https://github.com/elcoosp/rs-query/commit/1ed90f2de724e59b9c8403efd421dc5de0e2f0fa))
- all quoin-* crates from GitHub, not local paths by @elcoosp ([09e3462](https://github.com/elcoosp/rs-query/commit/09e3462d5083db0b0dfc2d9e8160a5551b9fe057))
- restore readme field + explicit include list by @elcoosp ([25bb44a](https://github.com/elcoosp/rs-query/commit/25bb44ab2306652ebbb1099d64d82db68c713704))
- support wasm32 in PoolRuntime task spawning by @elcoosp ([de39a6a](https://github.com/elcoosp/rs-query/commit/de39a6adb1a82f2e5292563e5867bb663fb2384e))
- support wasm32 in background GC loop by @elcoosp ([8619247](https://github.com/elcoosp/rs-query/commit/86192477e4364569ce2c2fb675e004d720c0460d))
- rename ToggleRsQueryDevtools by @elcoosp ([3e2543f](https://github.com/elcoosp/rs-query/commit/3e2543f345baba73c31d4bf33180c90850f62f04))
- pub crate cache entry no more for devtools by @elcoosp ([4a627b1](https://github.com/elcoosp/rs-query/commit/4a627b111b750e7f1b8668775daa649b9c97bd9a))
- pub cache by @elcoosp ([dbe8b47](https://github.com/elcoosp/rs-query/commit/dbe8b471714f6d3ebf1cdd113e0dc54acb0e66c6))
- compile devtools by @elcoosp ([e98a7e3](https://github.com/elcoosp/rs-query/commit/e98a7e38c0eea0a2be63ae15f204f1c4d3553292))
- improve deduplication test to use long-running task to avoid race by @elcoosp ([3a9b165](https://github.com/elcoosp/rs-query/commit/3a9b1658eb7ee04f55cb84bb0e8e6dcbac24a2f7))
- hooks by @elcoosp ([fb75f9d](https://github.com/elcoosp/rs-query/commit/fb75f9de9214bd14bfa7d3f05fe0f35ff76ddc35))
- add default features by @elcoosp ([4fb8f7e](https://github.com/elcoosp/rs-query/commit/4fb8f7eb6e47e21667c2d1094d1fdcddd10cdc96))
- working hooks by @elcoosp ([94a69c8](https://github.com/elcoosp/rs-query/commit/94a69c895de9c3f9d90b76a88cddfd2bf6632630))
- compiling crate by @elcoosp ([d43addf](https://github.com/elcoosp/rs-query/commit/d43addfbee65d3842b8323a55cce55589b538a0d))
- adjust trait bounds for structural sharing and infinite query support by @elcoosp ([69150c6](https://github.com/elcoosp/rs-query/commit/69150c69a4088f3f5a84fd393f1506557126f3c8))
- tests by @elcoosp ([e7450e5](https://github.com/elcoosp/rs-query/commit/e7450e5ad796f8248ed639b017ffc1267133de19))
- compiling by @elcoosp ([2e1825c](https://github.com/elcoosp/rs-query/commit/2e1825c9d6b3fa9493a627463c73bbea3db9c12b))
- compilation errors by @elcoosp ([08b1ce7](https://github.com/elcoosp/rs-query/commit/08b1ce7f3bff40314bded86bb9d700e3480b5855))
- use Arc arguments to resolve lifetime error by @elcoosp ([905d7a1](https://github.com/elcoosp/rs-query/commit/905d7a138799e6820ec09ef7fa3b7e51644000ff))
- correct JoinError pattern matching and simplify sharing by @elcoosp ([61bce5a](https://github.com/elcoosp/rs-query/commit/61bce5a7bd543cd00c6a57a9a7889d6da4efd2a0))
- resolve compilation errors in refetch interval implementation by @elcoosp ([93043d4](https://github.com/elcoosp/rs-query/commit/93043d48864e9066865fa74f4616e0a0d36b0604))
- focus_manager filename by @elcoosp ([6e7427a](https://github.com/elcoosp/rs-query/commit/6e7427a2d99a1d527c1bb18ead5af24a6d796806))
- resolve move errors by using downcast_ref and clone by @elcoosp ([0959ebb](https://github.com/elcoosp/rs-query/commit/0959ebb5c56430da66efbe7befaf74fd8ab3c059))
- resolve infinite query compilation errors and borrow conflicts by @elcoosp ([ff2dd1c](https://github.com/elcoosp/rs-query/commit/ff2dd1c0afde226e7a2ea98e6e0ddd140fffc066))
- export QueryStateUpdate and QueryStateVariant from observer module by @elcoosp ([272e690](https://github.com/elcoosp/rs-query/commit/272e6906797781406f0f130af3f96f5afef03397))

### Dependencies
- astro-takumi#4bb9e47 jsdom→optional, .ts sources, -jsdom devDep by @elcoosp ([bfbd919](https://github.com/elcoosp/rs-query/commit/bfbd919985dace71ae115ed916c5a5f84e38edb7))
- update Cargo.lock for wasm-bindgen-futures by @elcoosp ([6f02932](https://github.com/elcoosp/rs-query/commit/6f029327e76fb534164119f5c1c38fff82f55d34))
- update Cargo.lock by @elcoosp ([39bce6f](https://github.com/elcoosp/rs-query/commit/39bce6f764f2cdaa32584bc2c4210b8deca47186))
- update deps, adopt edition 2024 and pedantic lints by @elcoosp ([a517524](https://github.com/elcoosp/rs-query/commit/a517524e9720f7c411f43fcf98e032cad0cebac2))

### Documentation
- point documentation field to Vercel site by @elcoosp ([661b5fa](https://github.com/elcoosp/rs-query/commit/661b5fa19b45cbdf3112dd0fa3e21f1ed264d853))
- enforce SKILL.md rules (jsDelivr logos, absolute links, remove license sections) by @elcoosp ([78d8272](https://github.com/elcoosp/rs-query/commit/78d827266c46d088eabf271d1103e5f92907c54f))
- write reactive application integration tutorial by @elcoosp ([d676872](https://github.com/elcoosp/rs-query/commit/d6768726f26986df4fc19170f52fef49671b7f77))
- write headless query entry tutorial by @elcoosp ([929039a](https://github.com/elcoosp/rs-query/commit/929039a81b932d7fea6789e643832f3eaa4731ea))
- compile updater function reference index by @elcoosp ([384e0f4](https://github.com/elcoosp/rs-query/commit/384e0f40e94bffa01118fe2c587353afd0bf248b))
- compile runtime seam contract reference index by @elcoosp ([ce1951a](https://github.com/elcoosp/rs-query/commit/ce1951a2b536800d19db00d17a49fa0dc4c731d6))
- compile state machine variant reference index by @elcoosp ([c61726d](https://github.com/elcoosp/rs-query/commit/c61726d1f61c94d23616ffc4a233e81834c90289))
- compile query option matrix reference index by @elcoosp ([582bb10](https://github.com/elcoosp/rs-query/commit/582bb109a9cb79b778092c396fb221c5a942c5d2))
- compile dehydration payload reference index by @elcoosp ([0e341a5](https://github.com/elcoosp/rs-query/commit/0e341a581856f0cf8a4dbe7e22aae13ea7b5c25d))
- compile observer result flag reference index by @elcoosp ([f5f5c97](https://github.com/elcoosp/rs-query/commit/f5f5c97a3ceee99e6bf0ed3ce138835daa61023c))
- compile mutation hook state reference index by @elcoosp ([db81dbb](https://github.com/elcoosp/rs-query/commit/db81dbb3a3885ee71a3ee860dc3fd36ca13c93f2))
- compile query filter option reference index by @elcoosp ([95d83b9](https://github.com/elcoosp/rs-query/commit/95d83b9f9776777e25e7549e7f7d272b5c411591))
- compile infinite pagination data reference index by @elcoosp ([719db45](https://github.com/elcoosp/rs-query/commit/719db4505edd5a6bdd5200036d3f9df5aa311135))
- compile framework adapter token reference index by @elcoosp ([80e20fe](https://github.com/elcoosp/rs-query/commit/80e20fe4dc039054a0c1373be14d2ac95f0724f1))
- compile error classification reference index by @elcoosp ([d19525a](https://github.com/elcoosp/rs-query/commit/d19525ad44335ab68338f76cd139e5f4b21caa75))
- compile query client api reference index by @elcoosp ([5626112](https://github.com/elcoosp/rs-query/commit/5626112347c7ad49fd34903b21c45767143be6e1))
- write streamed query reducer task guide by @elcoosp ([562e4e7](https://github.com/elcoosp/rs-query/commit/562e4e7896b606e79ec2885c7cde0e25dcebb2a9))
- write query execution primitives task guide by @elcoosp ([354be10](https://github.com/elcoosp/rs-query/commit/354be10e6cefbbbe3a3ad12443173f1c1293261b))
- write storage persister integration task guide by @elcoosp ([412065e](https://github.com/elcoosp/rs-query/commit/412065e4e025fd080b35e52385beb1a2082a1aa1))
- write offline network state management task guide by @elcoosp ([86fd5d0](https://github.com/elcoosp/rs-query/commit/86fd5d029c517776a2c386504403ea4dfbf26970))
- write mutation execution task guide by @elcoosp ([317aea6](https://github.com/elcoosp/rs-query/commit/317aea6530bb0b09b9f5cacadb3e3825e31130a6))
- write query filtering mechanics task guide by @elcoosp ([498effd](https://github.com/elcoosp/rs-query/commit/498effd58b8e0db127eefc64bd9786b95edfb6a1))
- write workspace checkout installation task guide by @elcoosp ([39936f2](https://github.com/elcoosp/rs-query/commit/39936f20c29066a455675f7151fa89d6260c19e0))
- write infinite query pagination task guide by @elcoosp ([c0c5be7](https://github.com/elcoosp/rs-query/commit/c0c5be78875ce626616a8bdefd8ba940c35efa15))
- write state dehydration/hydration task guide by @elcoosp ([55a5e63](https://github.com/elcoosp/rs-query/commit/55a5e63095bc237ac88bd7e31c8367adeb0e7e0a))
- write reactive framework hook task guide by @elcoosp ([c087681](https://github.com/elcoosp/rs-query/commit/c0876817092779ce504ef2ca80d2aa96fa6811c4))
- write devtools configuration task guide by @elcoosp ([3a30334](https://github.com/elcoosp/rs-query/commit/3a303346f708f49f874d06b1ea59dd2dc300aa11))
- write tanstack v5 parity contract overview document by @elcoosp ([a408c69](https://github.com/elcoosp/rs-query/commit/a408c698afbd3d28b498154b238fae0bb7d811d7))
- write query caching mechanics overview document by @elcoosp ([afbb06f](https://github.com/elcoosp/rs-query/commit/afbb06f83b53e60aecc30435b11d6b83da31bf0b))
- write architecture concepts overview document by @elcoosp ([ce62e57](https://github.com/elcoosp/rs-query/commit/ce62e5780bc4598246050be0fcbdfa083e62d33e))
- add custom route conflict safe 404 handler page by @elcoosp ([0e20bf3](https://github.com/elcoosp/rs-query/commit/0e20bf31399673434bf6b9eadea44cc601bdedca))
- compile curated llms discoverability index map by @elcoosp ([b6c93c5](https://github.com/elcoosp/rs-query/commit/b6c93c511e941b561f6bb883b7b5296d39ecad95))
- compile stripped flat prose representation of full corpus by @elcoosp ([a002d33](https://github.com/elcoosp/rs-query/commit/a002d33bdf33ab336b2d2333dd53b7b4573be953))
- record visual route walk and defect fix matrix by @elcoosp ([ecfa15c](https://github.com/elcoosp/rs-query/commit/ecfa15c2080dacbcfdcc00b21b49c3a8d0f878f6))
- adr 0007 force emnapi alignment to 1.11.1 by @elcoosp ([5c23683](https://github.com/elcoosp/rs-query/commit/5c236838f4e1cd680b56652e90f11f8577317d49))
- adr 0006 lay out github pages deployment routing metrics by @elcoosp ([1390afc](https://github.com/elcoosp/rs-query/commit/1390afc145f575af1a7371507cdd6a7f7dfa18f4))
- adr 0005 enforce zero warning log policy by @elcoosp ([4e7159c](https://github.com/elcoosp/rs-query/commit/4e7159cfec73b2f7446995776470c16acd5599b1))
- adr 0004 select starlight plugins for a rust library surface by @elcoosp ([eddb9dd](https://github.com/elcoosp/rs-query/commit/eddb9dd08f00815447fe8dab4856b2ca6515057f))
- adr 0003 maintain shiki expressive code highlighting default by @elcoosp ([6ed186e](https://github.com/elcoosp/rs-query/commit/6ed186e6e7a3661eb751cbaf80718810d04774a7))
- adr 0002 generate optimized logo raster variants by @elcoosp ([8aa8175](https://github.com/elcoosp/rs-query/commit/8aa81754b3f0320ae4da0649f0492dbac9e38de2))
- adr 0001 pin typescript version to 6.x by @elcoosp ([cb9e25b](https://github.com/elcoosp/rs-query/commit/cb9e25b9599006cff98abd7c0f55337b9ed27046))
- add agent playbook and stack configuration matrix by @elcoosp ([6631903](https://github.com/elcoosp/rs-query/commit/6631903b70c0ea04fc31581a660790065fd97ee8))
- update README for quoin portability and new API by @elcoosp ([96c1346](https://github.com/elcoosp/rs-query/commit/96c13465f651dcc98fa538eb2c5cb949ab77ce34))
- add 0.2.0 release notes for quoin migration by @elcoosp ([976fc95](https://github.com/elcoosp/rs-query/commit/976fc9598aca0453f2725924f222d2f506af0f1c))
- rewrite docs for the expanded feature surface by @elcoosp ([3e23380](https://github.com/elcoosp/rs-query/commit/3e233809879b05c342b7883aaf05da415b7290c8))
- add generated changelog for 0.1.0 by @elcoosp ([a1d00ae](https://github.com/elcoosp/rs-query/commit/a1d00ae9630c544d1bfee55e8c3f6a58d6cbb2e0))
- up readme by @elcoosp ([0979edc](https://github.com/elcoosp/rs-query/commit/0979edc4c87c293c2b068d32d48ce7536392977a))

### Features
- bigger logo (160) + name (52) via toolkit 0.3.0 + accentOverride #3e3e74 by @elcoosp ([4bca4e9](https://github.com/elcoosp/rs-query/commit/4bca4e983ebd25f96da3ba0274eeda6f79eedc93))
- JSON-LD + 404 card + robots.txt + .qa gitignore (toolkit 0.2.1) by @elcoosp ([5a7202d](https://github.com/elcoosp/rs-query/commit/5a7202d57b6c0385b0f57687c86056fa269a4771))
- adopt @elcoosp/docs-toolkit — remove per-project OG boilerplate by @elcoosp ([f614e98](https://github.com/elcoosp/rs-query/commit/f614e98ae8cf67156dac2ce0deb7b93d93bb11c7))
- per-page OG images via astro-takumi fork (linkedom backend) by @elcoosp ([49e8b03](https://github.com/elcoosp/rs-query/commit/49e8b035319abb3d3a5eb4d3b5cb479a6040b643))
- add runtime-free watch cell by @elcoosp ([87b3742](https://github.com/elcoosp/rs-query/commit/87b3742cbbf1938bcee0fe0c84c99bdf60a03a01))
- derive PartialEq for QueryState and MutationState by @elcoosp ([0593afa](https://github.com/elcoosp/rs-query/commit/0593afa6499eb80f0279add2668ca130432208b7))
- add QueryRuntime seam, PoolRuntime, and SpawnedTask by @elcoosp ([48b80ca](https://github.com/elcoosp/rs-query/commit/48b80ca515896a79b0b6f99b40ec7a0fc31a2102))
- add runtime-free bounded broadcast channel by @elcoosp ([fcc2239](https://github.com/elcoosp/rs-query/commit/fcc22394200468b9430534fc75c6ca237fe188e2))
- update persister.rs by @elcoosp ([1f1ba91](https://github.com/elcoosp/rs-query/commit/1f1ba91857778d48af8a4508b65b0537d89890c9))
- update persist_client.rs by @elcoosp ([21f4b3b](https://github.com/elcoosp/rs-query/commit/21f4b3b20b4e7563cba469a622b74bbcf6730d2c))
- update options.rs by @elcoosp ([9d37cd6](https://github.com/elcoosp/rs-query/commit/9d37cd6f8559cc75b0add0d6bb727d1c54fd0a3d))
- update notify_manager.rs by @elcoosp ([4edea89](https://github.com/elcoosp/rs-query/commit/4edea89bae3853ab77f800801a3f6cca4aa07a49))
- update mutation_observer.rs by @elcoosp ([fa18aaa](https://github.com/elcoosp/rs-query/commit/fa18aaa5f51ab2205514f5b54cf14573e986358e))
- update key.rs by @elcoosp ([46254fe](https://github.com/elcoosp/rs-query/commit/46254fea5b3569a1774306f17550b833f31a0279))
- update infinite_executor.rs by @elcoosp ([af9c0f1](https://github.com/elcoosp/rs-query/commit/af9c0f1a4d3459a9dede727b3191fa63a9e829de))
- update focus_manager.rs by @elcoosp ([649cf7d](https://github.com/elcoosp/rs-query/commit/649cf7d470dda78e3183c18b21987070096e14c5))
- update error.rs by @elcoosp ([3472123](https://github.com/elcoosp/rs-query/commit/34721233b3fe33e47de3ecc290c8eba8a1984527))
- update cancellation.rs by @elcoosp ([09a1803](https://github.com/elcoosp/rs-query/commit/09a18032b51e4bb97ff90ec59facb5d9320745af))
- re-export new public API and document every item by @elcoosp ([780c53e](https://github.com/elcoosp/rs-query/commit/780c53ebd879d466df36ea8abfd55b56bc8ddcdf))
- add GPUI hooks for queries and mutations by @elcoosp ([df5ff6a](https://github.com/elcoosp/rs-query/commit/df5ff6a98b63423ac591c0dcbd0b8e77871ac924))
- add dehydrate and hydrate parity helpers by @elcoosp ([fc3a827](https://github.com/elcoosp/rs-query/commit/fc3a827a38b1991e75c1135b08c182352c3afc77))
- sync query and mutation caches across clients by @elcoosp ([84f7734](https://github.com/elcoosp/rs-query/commit/84f773439bae5e2cb6f0c2532f2cc1837de7a6ad))
- expose mutation cache and expand QueryClient surface by @elcoosp ([7116814](https://github.com/elcoosp/rs-query/commit/7116814e587955ccbf848d84912575e4e54dab30))
- add QueriesObserver and use_queries hook by @elcoosp ([867f390](https://github.com/elcoosp/rs-query/commit/867f3903f75c85c5b0a2d88fc4431fc4bc140c56))
- expand observer result flags and state exports by @elcoosp ([a42f8a7](https://github.com/elcoosp/rs-query/commit/a42f8a76dec7d64ade979badcf108988fa196d75))
- add scoped mutations, retries and callback ordering by @elcoosp ([38fa954](https://github.com/elcoosp/rs-query/commit/38fa954a52968b5060b14116f52b8d182d427c34))
- add mutation cache with scoped serial runs by @elcoosp ([d7e9a0c](https://github.com/elcoosp/rs-query/commit/d7e9a0c1fa728d88f154db7f2514bb543ff3a8fe))
- add streamed query support by @elcoosp ([23d25bb](https://github.com/elcoosp/rs-query/commit/23d25bbf8254ada3eb9a3ba961e72cbd8831aae9))
- complete infinite query fetch and page handling by @elcoosp ([a971891](https://github.com/elcoosp/rs-query/commit/a9718916f2789436921185fb5105e09a4dcf396b))
- align fetch execution with TanStack semantics by @elcoosp ([55f8053](https://github.com/elcoosp/rs-query/commit/55f805391d26573a04e23fae7e5407241694ba16))
- extend query state and fetch metadata by @elcoosp ([84bd023](https://github.com/elcoosp/rs-query/commit/84bd0235ebf401c82d03db243d587a475a173b41))
- add functional updater helpers by @elcoosp ([9b0ed60](https://github.com/elcoosp/rs-query/commit/9b0ed60fd273d04efc0381ba8282bb1d2727b483))
- add pluggable sleep provider for deterministic timers by @elcoosp ([f53178c](https://github.com/elcoosp/rs-query/commit/f53178c4c1e0db92af3af0795b489254d93678c8))
- add shared retry core for queries and mutations by @elcoosp ([1ede912](https://github.com/elcoosp/rs-query/commit/1ede912051a119736244104f1b22ce4e902408a1))
- add refetch filters and mutation matching by @elcoosp ([912a492](https://github.com/elcoosp/rs-query/commit/912a492243fd48c39145f757290dcf9315d36523))
- add mutation status and snapshot state types by @elcoosp ([b4f662f](https://github.com/elcoosp/rs-query/commit/b4f662fee390cfb4dc8ef7c1a0ba0a38b9716560))
- add NetworkMode for offline-aware fetching by @elcoosp ([665d5fd](https://github.com/elcoosp/rs-query/commit/665d5fda6f18e051412bd00cfa9d057e7c19ad90))
- implement structural sharing with opt-in PartialEq requirement by @elcoosp ([767956b](https://github.com/elcoosp/rs-query/commit/767956b00e18e790067fcb23f741e48857daea68))
- add initialData and placeholderData support with QueryObserver::from_query by @elcoosp ([2c941fb](https://github.com/elcoosp/rs-query/commit/2c941fb56eacd0e561196e41d771d40a72d53df7))
- complete passing test suite by @elcoosp ([9c3831d](https://github.com/elcoosp/rs-query/commit/9c3831dd868c795f3aed4d6245232abf40f56e43))
- add devtools by @elcoosp ([542f2fd](https://github.com/elcoosp/rs-query/commit/542f2fd995059e41722931cd2b1df29d398b5385))
- add AbortHandle-based query cancellation by @elcoosp ([6593142](https://github.com/elcoosp/rs-query/commit/659314283cc0dc4e3c5d9a407926476b2446b74a))
- add structural sharing for cache updates by @elcoosp ([4d1afb7](https://github.com/elcoosp/rs-query/commit/4d1afb762152b0ec27f847fc1adadc4a6004fe93))
- add refetch_interval and focus refetch support by @elcoosp ([6983273](https://github.com/elcoosp/rs-query/commit/698327329df3a92d8c4aee401195373b4e713093))
- add initialData, placeholderData, and select by @elcoosp ([d8bd761](https://github.com/elcoosp/rs-query/commit/d8bd761d63382af7c8449f3067a759cae91aadc7))
- add InfiniteQuery and infinite data support by @elcoosp ([3ba3645](https://github.com/elcoosp/rs-query/commit/3ba36457cac698fc8108542086841d2e913a48ab))
- add optimistic updates with on_mutate and rollback by @elcoosp ([ff56bcc](https://github.com/elcoosp/rs-query/commit/ff56bcc62c88ccfc4b84fa0904e52d8d8d8b76eb))
- add QueryObserver and reactive state subscriptions by @elcoosp ([1a81921](https://github.com/elcoosp/rs-query/commit/1a819216c56f3b201c26a5b7137707d147a2045c))
- add QueryObserver and reactive state subscriptions by @elcoosp ([49db226](https://github.com/elcoosp/rs-query/commit/49db2269e6079ffc17d52414e3a6e2904c71e5d3))
- optimize query execution with DashMap, cached keys, futures-timer, and remove Tokio runtime by @elcoosp ([711d9dd](https://github.com/elcoosp/rs-query/commit/711d9dd4f83f2e3fcc617e796da7d24a6b2995c3))
- update gpui dep of https://docs.rs/rs-query/latest/rs_query/ by @elcoosp ([b0d2170](https://github.com/elcoosp/rs-query/commit/b0d2170367c7f08f66dce6f8b5fcad1b9c6a9d80))

### Miscellaneous
- inject missing metadata and point to root README by @elcoosp ([7bde42f](https://github.com/elcoosp/rs-query/commit/7bde42f873dd8bf2fd8e4d486ea577497287cb62))
- inject curated keywords by @elcoosp ([5892007](https://github.com/elcoosp/rs-query/commit/589200798436955643d2ee21b7fe354f7f92b899))
- bump to 0.2.3 — TechArticle restored, home breadcrumb skip by @elcoosp ([8bddc74](https://github.com/elcoosp/rs-query/commit/8bddc74535d1ddc31a66dcc02a0a999ed63abd97))
- add bootstrap-artifacts.mjs by @elcoosp ([dc740e9](https://github.com/elcoosp/rs-query/commit/dc740e9440e7277bcb5a2cb218a19934f8342e25))
- add qa-components.mjs by @elcoosp ([03ba522](https://github.com/elcoosp/rs-query/commit/03ba522653e43e6cbf82b2df7f25b3a460ddf1ee))
- more docs by @elcoosp ([89ef1d8](https://github.com/elcoosp/rs-query/commit/89ef1d8a4ca229863f4070262c06644ec40582e7))
- provide typescript client global module references by @elcoosp ([6b9d565](https://github.com/elcoosp/rs-query/commit/6b9d5656d2a7804f79db34d4693fe747f17d89cd))
- expose default internationalization root object by @elcoosp ([0d092ef](https://github.com/elcoosp/rs-query/commit/0d092ef017fb1a1097303dce3d8a0bd1db5dec74))
- add automated testing script for search index discovery by @elcoosp ([f133963](https://github.com/elcoosp/rs-query/commit/f133963aec0111a4ba134319a75cd23b40f1f97a))
- add visual testing pipeline script using playwright by @elcoosp ([bf842f7](https://github.com/elcoosp/rs-query/commit/bf842f7c9eb083f2deff2e6d7313247693fd6d60))
- add script to compile downstream agent context documents by @elcoosp ([be40d6e](https://github.com/elcoosp/rs-query/commit/be40d6e0cfeee177ffc13354f438bd564b227b03))
- expose default permissive search index instructions by @elcoosp ([374b1b6](https://github.com/elcoosp/rs-query/commit/374b1b665c87394d2a5897ed4b4edf6759ae38ce))
- configure markdownlint exclusions and custom rules by @elcoosp ([57480c4](https://github.com/elcoosp/rs-query/commit/57480c4ef2cb96e7a9fae5d280407dc34ceb286f))
- add gitignore rules for astro and qa artifacts by @elcoosp ([e3c7fb9](https://github.com/elcoosp/rs-query/commit/e3c7fb92cd2d5ed4a7f3d77cf90ebe0e23c76166))
- logo by @elcoosp ([ee416b8](https://github.com/elcoosp/rs-query/commit/ee416b8db255eb34bb29d4a9663e66135f39609d))
- add cmt.sh by @elcoosp ([c41827c](https://github.com/elcoosp/rs-query/commit/c41827c6b08f8a158d18ee497e2fb515a4793670))
- up readme by @elcoosp ([4d2b110](https://github.com/elcoosp/rs-query/commit/4d2b110fff235a74b70fc7bfdc93fd6701bc4c19))
- up readme by @elcoosp ([a8d05df](https://github.com/elcoosp/rs-query/commit/a8d05dfb31ac56542fbba68dfb9db3ba07d2f0be))
- readme by @elcoosp ([5925f65](https://github.com/elcoosp/rs-query/commit/5925f654d591e64d20b55143ea6b64e988d37620))

### Refactor
- replace TokioSleepProvider with QuoinSleepProvider by @elcoosp ([3efe57c](https://github.com/elcoosp/rs-query/commit/3efe57c8db4e337874a7e33a513f11a0946b1998))
- update cancellation token import by @elcoosp ([32b3e7c](https://github.com/elcoosp/rs-query/commit/32b3e7cd759134321bac46d7ba61c9a426ca35af))
- migrate to quoin_core cancellation and injectable timer by @elcoosp ([0253a76](https://github.com/elcoosp/rs-query/commit/0253a766589eca5e55909ea726d9aad9f1348089))
- return SpawnedTask for query executions by @elcoosp ([e6bf09b](https://github.com/elcoosp/rs-query/commit/e6bf09b2649d35ef36130d12e4cb7447ac612e6f))
- migrate save loop to runtime and SpawnHandle by @elcoosp ([5a146fe](https://github.com/elcoosp/rs-query/commit/5a146fefa0630d538e88b554769df25ed9bc9a46))
- migrate to custom bus by @elcoosp ([78e18be](https://github.com/elcoosp/rs-query/commit/78e18be2ad9d7c2a2f9cb6442274e1d07a9cfe37))
- replace tokio watch with custom watch cell by @elcoosp ([fa9ab92](https://github.com/elcoosp/rs-query/commit/fa9ab920fe3e601634f54823e783eab19c881b0d))
- return SpawnedTask and use bus by @elcoosp ([7242c68](https://github.com/elcoosp/rs-query/commit/7242c68532628327387a3186c72c03e7e4de4e77))
- migrate to bus and watch-based scope release by @elcoosp ([99c4b4a](https://github.com/elcoosp/rs-query/commit/99c4b4a3d255ed7c10a431de99daab30554825b4))
- use runtime sleep and watch for scope serialization by @elcoosp ([8f55776](https://github.com/elcoosp/rs-query/commit/8f5577624b11af29e9cd1b388b3ca2e60c2298dd))
- update exports and docs for quoin migration by @elcoosp ([549afc7](https://github.com/elcoosp/rs-query/commit/549afc7a6c1276d4ecd78f7bfb27c6ee2a0b059e))
- use runtime spawn and custom page request slot by @elcoosp ([884389a](https://github.com/elcoosp/rs-query/commit/884389a51f9a4640dec9d5fb70d772483f730158))
- rewrite bindings for quoin ReactiveContext and signals by @elcoosp ([5c45cbf](https://github.com/elcoosp/rs-query/commit/5c45cbf784ecd24d57055481ad78441440ab4d2a))
- migrate spawn functions to QueryRuntime and return SpawnedTask by @elcoosp ([e41cf37](https://github.com/elcoosp/rs-query/commit/e41cf3700c64d2465fd839fc81c2f0811560c19d))
- remove tokio JoinError conversion by @elcoosp ([ed6f9ec](https://github.com/elcoosp/rs-query/commit/ed6f9ecf53a198f824a9af5a44a4af55f86f54f0))
- replace gpui panel with headless state and collector by @elcoosp ([efc5556](https://github.com/elcoosp/rs-query/commit/efc555641f4c60a0a15ec77094435e915dc6996e))
- introduce QueryRuntime seam and replace tokio primitives by @elcoosp ([6dcd90d](https://github.com/elcoosp/rs-query/commit/6dcd90dbfc6a14ec5dca622525343a1efaf26f00))
- migrate to quoin_core::CancellationToken by @elcoosp ([625eb11](https://github.com/elcoosp/rs-query/commit/625eb11422adf5448971a0d7a83a26b00a2dff31))
- replace tokio channels with runtime-agnostic bus and watch by @elcoosp ([40abb4a](https://github.com/elcoosp/rs-query/commit/40abb4a2381459814ca46f3c25cac2968eaf6089))
- tidy gpui imports and satisfy pedantic clippy by @elcoosp ([d42e8c9](https://github.com/elcoosp/rs-query/commit/d42e8c9baeec6c3370088ac6ad3ae9d1f0dd6632))
- rework structural sharing around Arc values by @elcoosp ([f1c76c7](https://github.com/elcoosp/rs-query/commit/f1c76c70ceafb9bbbd2487c3be7ca5c25e13a143))

### Testing
- working tests by @elcoosp ([3aab83c](https://github.com/elcoosp/rs-query/commit/3aab83c62655f22f283ee7fa235c0c996b73d060))
- add more tests by @elcoosp ([afe5c4d](https://github.com/elcoosp/rs-query/commit/afe5c4dbc1c5cf2d02c6b95256e892e1563ddd27))
- add more tests by @elcoosp ([aec995d](https://github.com/elcoosp/rs-query/commit/aec995d619e368dfb1a4794e74f20e82400cc33f))
- add more testing by @elcoosp ([65a803f](https://github.com/elcoosp/rs-query/commit/65a803fbd44250dec622fc723a42339dab39ae13))
- pass tests by @elcoosp ([6097b7f](https://github.com/elcoosp/rs-query/commit/6097b7f622fd6205b179d55c22791bc4ae78f521))

### build
- declare rigorous type checking profile configurations by @elcoosp ([fa36fcd](https://github.com/elcoosp/rs-query/commit/fa36fcd0db782e660a38c64f5fed048536675d85))
- build content collection mappings and schemas by @elcoosp ([dbe410d](https://github.com/elcoosp/rs-query/commit/dbe410dfbfab527fbda8d039822c5988edeeb4f9))
- generate lockfile for astro starlight and workspace targets by @elcoosp ([9b87217](https://github.com/elcoosp/rs-query/commit/9b872176a6236a77d349ce4516159fd7d494cc33))
- structure documentation engine metadata and dependencies by @elcoosp ([a9b624f](https://github.com/elcoosp/rs-query/commit/a9b624f7251aa3ea27f4c23c76f2818f435522c9))
- configure starlight documentation site and plugins by @elcoosp ([e26bc7f](https://github.com/elcoosp/rs-query/commit/e26bc7f9fe38d7a13b6465fb3c2d212ddcb07722))
- add wasm-bindgen-futures for wasm32 target by @elcoosp ([6f97902](https://github.com/elcoosp/rs-query/commit/6f97902dbae713717701a9ab74a14f430e08fce5))
- bump version to 0.2.0 and migrate dependencies to quoin by @elcoosp ([4c7c840](https://github.com/elcoosp/rs-query/commit/4c7c8403c8427beb6b493c4a76e3a6682324f1b5))
- update Cargo.lock for quoin migration by @elcoosp ([7394f3f](https://github.com/elcoosp/rs-query/commit/7394f3f36c5564497cbe0f2fcbdae0c17d9eb3f5))

### content
- docs + frontmatter YAML quoting fixes by @elcoosp ([271936e](https://github.com/elcoosp/rs-query/commit/271936e714398ae93651eb021715f9a9fb822b52))

### style
- cargo fmt --all + cargo clippy --fix by @elcoosp ([d53ebc4](https://github.com/elcoosp/rs-query/commit/d53ebc4e43ad5cdf23f1b92b0fd10245272d2a4b))
- inject brand accent parameters by @elcoosp ([827e8de](https://github.com/elcoosp/rs-query/commit/827e8defab319fd983ca637812ce3eab15590534))
- commit raw alpha trimmed light theme logo mark by @elcoosp ([4ea4638](https://github.com/elcoosp/rs-query/commit/4ea46386df95f9c7ad20f1a69184a917831d0da7))
- commit high-contrast dark theme logo plate by @elcoosp ([46dcd46](https://github.com/elcoosp/rs-query/commit/46dcd468a06ddc0d69e5d02ea6e0655d60b46030))
- commit canonical site favicon asset by @elcoosp ([d301d93](https://github.com/elcoosp/rs-query/commit/d301d9334f9e73da30706b2a0071edbaf8ff5851))
**Full Changelog**: https://github.com/elcoosp/rs-query/releases/tag/v0.2.1

## [0.2.0] - 2026-11-02

### Breaking Changes — quoin migration

- **gpui and tokio are gone from the dependency tree.** rs-query now depends only on
  `quoin-core` (+ `futures`); the reactive layer is generic over
  `quoin_core::ReactiveContext` and the runtime layer goes through the new object-safe
  `QueryRuntime` seam (`PoolRuntime` default: quoin's shared worker pool + portable
  timers; `ExecutorRuntime`: any framework executor). See
  `docs/quoin-migration-plan.md` for the full contract.
- `spawn_query(cx, client, query, callback)` → `spawn_query(client, query) ->
  SpawnedTask<QueryState<T>>` (awaitable, abortable, drop-cancels; panic-isolated
  fetches surface as custom error states). Same for `spawn_mutation`.
- `spawn_infinite_query(cx, query, _cb)` → `spawn_infinite_query(client, query)`; the
  page loop runs on the client's runtime with a capacity-1 request slot (drop-newest,
  as before).
- `hooks::*` return the framework's own signals (`C::Signal<...>`) instead of gpui
  entities; new bindings `use_query` / `use_mutation` / `use_infinite_query` join the
  existing activity hooks. Dropping a binding cancels its pump.
- `CancellationToken` is now quoin's (re-exported at the crate root); subscriptions
  return `bus::Receiver` (bounded broadcast with `Lagged` semantics); the online
  manager and stop signals use a runtime-free `watch` cell.
- `devtools` is headless (cache rows, timestamped transition timeline with filters,
  actions, production-wired collector); the gpui-component panel is removed.
- `QueryState`/`MutationState` derive `PartialEq` (signal write gating); removed
  `From<tokio::task::JoinError>`; `TokioSleepProvider` → `QuoinSleepProvider`
  (runtime-free); `QueryClient::with_runtime` / `QueryClient::with_context(&cx)` added;
  `QueryClient::subscribe_all()` added (client-wide update stream).

### Added
- `docs/quoin-migration-plan.md` — the migration contract (module-by-module, quoin
  changes, test strategy).
- 713 tests (from 704): the 41 gpui-based tests are ported to runtime/testkit-based
  equivalents; new binding, bus, watch, runtime and devtools suites run headless on
  quoin-testkit.

## [0.1.0] - 2026-11-01

### Bug Fixes

- Export QueryStateUpdate and QueryStateVariant from observer module
- Resolve infinite query compilation errors and borrow conflicts
- *(executor)* Resolve move errors by using downcast_ref and clone
- Focus_manager filename
- Resolve compilation errors in refetch interval implementation
- Correct JoinError pattern matching and simplify sharing
- *(sharing)* Use Arc arguments to resolve lifetime error
- Compilation errors
- Compiling
- Tests
- Adjust trait bounds for structural sharing and infinite query support
- Compiling crate
- Working hooks
- Add default features
- Hooks
- Improve deduplication test to use long-running task to avoid race
- Compile devtools
- Pub cache
- Pub crate cache entry no more for devtools
- Rename ToggleRsQueryDevtools

### Documentation

- Up readme
- Add TanStack Query parity port plan and research appendix
- Rewrite README for the TanStack-parity crate state

### Features

- Update gpui dep of https://docs.rs/rs-query/latest/rs_query/
- *(perf)* Optimize query execution with DashMap, cached keys, futures-timer, and remove Tokio runtime
- *(observers)* Add QueryObserver and reactive state subscriptions
- *(observers)* Add QueryObserver and reactive state subscriptions
- *(mutation)* Add optimistic updates with on_mutate and rollback
- *(infinite)* Add InfiniteQuery and infinite data support
- *(options)* Add initialData, placeholderData, and select
- *(refetch)* Add refetch_interval and focus refetch support
- *(sharing)* Add structural sharing for cache updates
- *(cancellation)* Add AbortHandle-based query cancellation
- Add devtools
- Complete passing test suite
- Add initialData and placeholderData support with QueryObserver::from_query
- Implement structural sharing with opt-in PartialEq requirement
- Add TanStack parity foundations (filters, retryer, managers)
- Complete QueryClient surface and executor TanStack semantics
- Mutation cache with scope serialization and full lifecycle
- Persistence, broadcast sync and hydration parity
- GPUI hooks, queries observer, streamedQuery and devtools upkeep

### Miscellaneous Tasks

- Readme
- Up readme
- Up readme
- Update deps, edition 2024, pedantic lint gates
- Add GitHub Actions CI/CD and git-cliff release changelog

### Tests

- Pass tests
- Add more testing
- Add more tests
- Add more tests
- Working tests
- Finish deterministic suites and coverage to 98.46%
<!-- generated by git-cliff -->
