# TypeScript support measurements

<!-- derived-from ./TEST-REACH-TARGET.md#evaluation -->

## Question

What does dagayn answer about TypeScript test reach, measured against the
coverage of a vitest run? These are the measurements of work item M1 of the
first-class languages plan (`docs/plans/FIRST-CLASS-LANGUAGES-TARGET.md`),
with T2's numbers (the same run on a `dagayn build --scip` graph) beside
them. They are report only: no floor and no CI gate.

## Corpora and runs

| Repository | Version | Tests | Coverage run |
|---|---|---|---|
| tstodon | `4694e9c` | 135 files, 504 tests, all pass | istanbul provider (workerd supports no other) |
| cfwdon `web-ui` | `9432966` | 53 files, 216 tests, all pass | v8 provider; istanbul gives the same numbers |

Each repository was copied to a scratch directory without its
`node_modules`, `.dagayn`, and `.wrangler`, then installed with
`pnpm install --frozen-lockfile` (Node 24.22.1) and given
`@vitest/coverage-istanbul@5.0.5` (and `@vitest/coverage-v8@5.0.5` for
web-ui), the version of the repositories' vitest 5.0.5. The runs:

```sh
# tstodon (two vitest projects: node, and workerd through @cloudflare/vitest-plugin)
vp test run --coverage.enabled --coverage.provider=istanbul \
  --coverage.reporter=json --coverage.reportsDirectory=coverage \
  '--coverage.include=packages/*/src/**/*.ts' '--coverage.exclude=**/generated/**'

# cfwdon/web-ui
vitest run --coverage.enabled --coverage.provider=v8 \
  --coverage.reporter=json --coverage.reportsDirectory=coverage-v8 \
  '--coverage.include=src/**'
```

`coverage.include` makes the report list files no test loads, with every
function at count 0. tstodon's generated Prisma client is excluded:
including it makes the instrumenter parse `query_compiler_fast_bg.wasm` as
JavaScript and fail the run, and the plan leaves generated code out of
every population anyway.

Graphs were built from the worktree's own build (`main` at `b98b3ff3`,
javascript extractor 13) with `dagayn build --repo COPY --force`, then
again with `--scip` (dagayn's linked tsrs indexer; on cfwdon rust-analyzer
indexes the Rust crates too). Before each measurement the graph metadata
read `last_build_type` `full` and `javascript=13`. cfwdon's graph is the
whole repository, as cfwdon is one git repository; every nearest test the
harness named for a web-ui function is a web-ui test.

## The reader

<!-- derived-from ./TEST-REACH-TARGET.md#evaluation -->

`eval/run_test_reach_eval.py` reads an istanbul `coverage-final.json`
(`--coverage coverage-final.json --repo TS_REPO`). vitest's `json` reporter
writes that format from either provider. On web-ui the two providers list
the same 1,485 functions at the same declaration lines and agree on which
of them ran.

Coverage names almost every function `(anonymous_N)` (1,484 of 1,485 on
web-ui), so the reader matches on place, not name: a `Function` node of
TypeScript, TSX, or JavaScript matches the coverage function whose name
(`decl`) starts on the node's first line. A callback written on that same
line declares there too, further right, so the leftmost one is the node's
own. A function counts as run when its count is above zero.

| | tstodon | web-ui |
|---|---|---|
| Coverage functions | 2,074 | 1,485 |
| Distinct (file, declaration line) | 2,068 | 1,460 |
| ... with a production node at that line | **1,034** | **483** |
| ... with a node of a test helper file at that line (`src/ui/test/`) | 0 | 15 |
| ... inside a node (callbacks, handlers in a component) | 714 (658 ran) | 934 (271 ran) |
| ... outside every node | 320 (310 ran) | 28 (22 ran) |
| Production nodes with no coverage function at their line | 29 | 0 |

The rows outside every node are functions the graph has no node for. On
tstodon they are the Hono route handlers (`announcementRoutes.get(path,
async (c) => {...})`) and `app.notFound`'s arrow: anonymous arrows passed
at module scope. On web-ui they are arktype `.pipe((value) => ...)` arrows
of module-level parser constants. Neither is in any population, so no
number below scores them.

tstodon's 29 nodes with no coverage function are `X.parse` members of
exported objects (`parse: schemaResult(schema)`, the nodes #238 added). The
member is a call, not a function literal, so istanbul has nothing to count
there, and the reader cannot score these nodes. tstodon's other
production nodes outside the population are the generated Prisma client
(1,602), `e2e/` (54, run by `tsx`, not vitest), and `prisma.config.ts`.

## M1: test reach against coverage

<!-- derived-from #the-reader -->

| Repository | Graph | Functions (ran) | Called untested | Ran, called untested | `no_callers` / `no_test_path` / `beyond_limit` (ran) | Untested precision | Recall | Tested precision |
|---|---|---|---|---|---|---|---|---|
| tstodon | plain | 1,034 (978) | 592 | **538** | 7 / 476 / 55 | 0.091 | 0.964 | 0.995 (442) |
| tstodon | `--scip` | 1,034 (978) | 589 | 535 | 7 / 473 / 55 | 0.092 | 0.964 | 0.996 (445) |
| web-ui | plain | 483 (312) | 95 | **15** | 3 / 12 / 0 | 0.842 | 0.468 | **0.765** (388) |
| web-ui | `--scip` | 483 (312) | 95 | 15 | 3 / 12 / 0 | 0.842 | 0.468 | 0.765 (388) |

TypeScript keeps the 4-hop limit. With 8 hops (read from the same run:
`beyond_limit` has the hop count), tstodon's ran-and-called-untested falls
from 538 to 483 at no cost: none of the 55 failed to run, so tested
precision goes from 0.995 (442) to 0.996 (497). web-ui has no `beyond_limit` row. A run with
no limit needs a change to `caller_test_depth`, and was not made.

The two repositories fail in opposite directions:

- **tstodon: almost everything ran, and most of it is called untested.**
  978 of 1,034 functions ran (0.946), so an untested precision of 0.091
  only restates that the call graph reaches few of them, as testify's 0.615
  did for Go. Its worker tests are HTTP tests: 87 of 135 test files send a
  request through `SELF.fetch` from `cloudflare:test`, which no edge
  follows (below).
- **web-ui: what is called tested often did not run.** 91 of the 388
  functions called tested did not run, a tested precision of 0.765 against
  0.94 to 0.996 elsewhere. A component test renders a page, and the graph
  credits the page with every call in it, including the calls inside
  event handlers the test never fires and the routes it never visits
  ([below](#web-ui-functions-called-tested-that-did-not-run)).

`--scip` changes three answers on tstodon, all right:
`StreamHub.publish`, `StreamHub.sessionOf`, and `StreamHub.write` are now
reached from `index.test.ts`, because tsrs types `streamHub(env)` as the
Durable Object stub and settles `.publish(envelope)` on it. On web-ui it
changes none.

The plan asks for a floor file if tested precision is at least 0.90.
tstodon's is 0.996 and web-ui's 0.765. No floor is set: M1 is report
only, and neither repository has a coverage run in dagayn's CI.

### tstodon: why the functions that ran are called untested

<!-- derived-from #m1-test-reach-against-coverage -->

Every one of the 538 functions that ran and is called untested (plain
graph), by the first break between the test that ran it and the function.
The counts come from the graph: for each function, every node that
reaches it backward over `CALLS` and `REFERENCES` (no limit), and the File
nodes among them, whose module-level statements are the calls no function
node owns. For each File root, the top-level statement that makes the call
was read from source.

| First break | Ran, called untested | Checked by hand |
|---|---|---|
| The test sends a request (`SELF.fetch`); the function is under a Hono route handler registered at module scope | **470** | a seeded sample of 50 (below) |
| ... the handler is an anonymous arrow | 457 | |
| ... a named function or a handler factory (`reviewLink("Approved")`) | 10 | |
| ... middleware (`app.use`) | 3 | |
| A test 5 to 8 hops away (hop limit 4) | 55 | |
| A Workers runtime entry point, or code only under one | 6 | all |
| A Durable Object method called through its stub (`streamHub(env).publish(..)`) | 3 | all; `--scip` settles all 3 |
| A call at module load (`const PublicSchema = unitKind("Public")`) | 1 | yes |
| A shorthand member (`{ schema, wireState }`) that a test calls as `CollectionItem.wireState(..)` (#239) | 1 | yes |
| A method passed as a value (`items.filter(CollectionItem.isVisible)`) | 1 | yes |
| A function called through a table (`TITLES[type](name)`) | 1 | yes |

The 470 come from the graph rule above: the File roots of each are route
files (and `app.ts`, for middleware), and no test is among the nodes that
reach it, at any depth. For 4 of them (`listFromRow`,
`findListForAccount`, `StreamHub.fetch`, `StreamHub.serverSentEvents`) the
streaming Durable Object's handlers reach them too. They are the
functions a request runs, and a test reaches the worker only through a
request:
`SELF.fetch(url)` runs the worker's `default.fetch`, which calls
`app.fetch`, and Hono dispatches on the path to the handler. dagayn records
none of that: `SELF` is a runtime binding, and the handler is an argument
of `app.get`. The one test that calls `app.request` (`http.test.ts`) builds
its own Hono app.

The runtime entry points are the worker's `default.fetch` and its
`queue: handleQueue` member, `OutboxDeliveryWorkflow.run` (a Workflow) and
`persistOutboxTarget` under it, and `StreamHub.webSocketMessage` and
`StreamHub.webSocketClose`, which workerd calls on a Durable Object. The
three stub calls are `StreamHub.publish`, `StreamHub.sessionOf` (under
`publish` and `webSocketMessage`), and `StreamHub.write` (under `publish`,
and under the streaming route's SSE path, counted here because `--scip`
reaches it through `publish`).

Against T3's bar (at least 10 wrong answers from lazy `await import()`
routes and anonymous route arrows):

- **Lazy `await import()` routes: 0.** tstodon has no `await import(`
  outside its generated client.
- **Anonymous route arrows: 457 in the chain, 0 that T3 alone fixes.** No
  test reaches a route file by any edge, so an arrow with a node of its own
  would still have no test above it. What breaks these answers is the
  request: a test's `SELF.fetch` to the worker's `fetch`, and a route's
  registration to its handler.
- **Call-valued module constants (#239): 0 here.** The one #239 answer is a
  shorthand member (`wireState`). web-ui has 11 (below).

#### Hand check of the route rows

A seeded random sample: `random.Random(20261103).sample` of 50 from the
466 sorted names whose only File roots are route files (the other 4 of the
470 were read with the runtime rows). For each, the shortest backward chain
to the route file, the call site of each hop, and the top-level statement
were read from source. Every one ends in a route registration; 48 handlers
are anonymous arrows, one is a named handler, and one is a middleware
factory called at module load (`apiCors()`).

| # | Function | Route registration | Handler | Calls below it |
|---|---|---|---|---|
| 1 | `announcement-store.ts::addAnnouncementReaction` | `routes/announcements.ts:186` `announcementRoutes.put("/api/v1/announcements/:id/reactions/:name", (c) =>` | arrow | 2 |
| 2 | `domain-allow-store.ts::findDomainAllowById` | `routes/admin/domain-allows.ts:50` `adminDomainAllowRoutes.get("/api/v1/admin/domain_allows/:id", async (c) =>` | arrow | 1 |
| 3 | `endorsement-store.ts::unendorseAccount` | `routes/accounts/endorsements.ts:72` `accountEndorsementRoutes.post("/api/v1/accounts/:id/endorse", (c) => set...` | arrow | 2 |
| 4 | `filter-store.ts::insertFilter` | `routes/filters/v1.ts:47` `filterV1Routes.post("/api/v1/filters", async (c) =>` | arrow | 1 |
| 5 | `routes/notifications.ts::groupEnvelope` | `routes/notifications.ts:335` `notificationRoutes.get("/api/v2/notifications", async (c) =>` | arrow | 1 |
| 6 | `routes/notification-requests.ts::requestDocument` | `routes/notification-requests.ts:177` `notificationRequestRoutes.get("/api/v1/notifications/requests", async (c...` | arrow | 1 |
| 7 | `status-pin-store.ts::listPinnedStatuses` | `routes/accounts/profile.ts:48` `accountProfileRoutes.get("/api/v1/accounts/:id/statuses", async (c) =>` | arrow | 1 |
| 8 | `custom-emoji-store.ts::isValidShortcode` | `routes/admin/custom-emojis.ts:54` `adminCustomEmojiRoutes.post("/api/v1/admin/custom_emojis", async (c) =>` | arrow | 1 |
| 9 | `admin-domain-block-store.ts::domainDigest` | `routes/instance.ts:85` `instanceRoutes.get("/api/v1/instance/domain_blocks", async (c) =>` | arrow | 2 |
| 10 | `routes/admin/domain-blocks.ts::optionalComment` | `routes/admin/domain-blocks.ts:82` `adminDomainBlockRoutes.post("/api/v1/admin/domain_blocks", async (c) =>` | arrow | 1 |
| 11 | `inbox-handlers/like.ts::handleRemoteLike` | `routes/activitypub/inbox.ts:198` `inboxRoutes.post("/users/:username/inbox", (c) => handleInbox(c));` | arrow | 3 |
| 12 | `routes/admin/email-blocks.ts::emailDomainBlockDocument` | `routes/admin/email-blocks.ts:157` `adminEmailBlockRoutes.get("/api/v1/admin/email_domain_blocks", async (c) =>` | arrow | 1 |
| 13 | `routes/timelines.ts::timelineScopes` | `routes/timelines.ts:124` `timelineRoutes.get("/api/v1/timelines/public", async (c) =>` | arrow | 1 |
| 14 | `routes/scheduled-statuses.ts::notFound` | `routes/scheduled-statuses.ts:85` `scheduledStatusRoutes.get("/api/v1/scheduled_statuses/:id", async (c) =>` | arrow | 1 |
| 15 | `routes/statuses/engagement.ts::statusEngagementAccounts` | `routes/statuses/engagement.ts:78` `statusEngagementRoutes.get("/api/v1/statuses/:id/favourited_by", (c) =>` | arrow | 1 |
| 16 | `routes/admin/accounts.ts::setSensitized` | `routes/admin/accounts.ts:491` `adminAccountRoutes.post("/api/v1/admin/accounts/:id/sensitize", (c) => s...` | arrow | 1 |
| 17 | `conversation-store.ts::hideConversation` | `routes/conversations.ts:66` `conversationRoutes.delete("/api/v1/conversations/:id", async (c) =>` | arrow | 1 |
| 18 | `filter-match.ts::loadViewerFilters` | `routes/notifications.ts:59` `notificationRoutes.get("/api/v1/notifications", async (c) =>` | arrow | 1 |
| 19 | `report-store.ts::setReportAssigned` | `routes/admin/reports.ts:250` `adminReportRoutes.post("/api/v1/admin/reports/:id/assign_to_self", (c) =...` | arrow | 2 |
| 20 | `scheduled-status-store.ts::updateScheduledStatusAt` | `routes/scheduled-statuses.ts:101` `scheduledStatusRoutes.put("/api/v1/scheduled_statuses/:id", async (c) =>` | arrow | 1 |
| 21 | `filter-store.ts::filterRowFrom` | `routes/filters/v1.ts:35` `filterV1Routes.get("/api/v1/filters", async (c) =>` | arrow | 2 |
| 22 | `notification-store.ts::deleteRemoteNotifications` | `routes/activitypub/inbox.ts:198` `inboxRoutes.post("/users/:username/inbox", (c) => handleInbox(c));` | arrow | 4 |
| 23 | `routes/oauth/shared.ts::clientCredentials` | `routes/oauth/revoke.ts:11` `oauthRevokeRoutes.post("/oauth/revoke", async (c) =>` | arrow | 2 |
| 24 | `routes/admin/accounts.ts::adminAccountFilters` | `routes/admin/accounts.ts:115` `adminAccountRoutes.get("/api/v1/admin/accounts", async (c) =>` | arrow | 1 |
| 25 | `notification-policy-store.ts::findNotificationRequest` | `routes/notification-requests.ts:212` `notificationRequestRoutes.get("/api/v1/notifications/requests/:id", asyn...` | arrow | 1 |
| 26 | `notification-store.ts::clearNotifications` | `routes/notifications.ts:175` `notificationRoutes.post("/api/v1/notifications/clear", async (c) =>` | arrow | 1 |
| 27 | `report-store.ts::setReportResolved` | `routes/admin/accounts.ts:413` `adminAccountRoutes.post("/api/v1/admin/accounts/:id/action", async (c) =>` | arrow | 1 |
| 28 | `routes/trends.ts::trendingTagDocuments` | `routes/trends.ts:84` `trendRoutes.get("/api/v1/trends", trendingTagsResponse);` | named handler | 2 |
| 29 | `status-timeline-store.ts::listBookmarkedStatuses` | `routes/statuses/interactions.ts:406` `statusInteractionRoutes.get("/api/v1/bookmarks", async (c) =>` | arrow | 1 |
| 30 | `oauth-app-store.ts::findOAuthApplicationByClientId` | `routes/oauth/authorize.ts:156` `oauthAuthorizeRoutes.get("/oauth/authorize", async (c) =>` | arrow | 2 |
| 31 | `tag-store.ts::normalizeTag` | `routes/tags.ts:117` `tagRoutes.get("/api/v1/tags/:name", async (c) =>` | arrow | 1 |
| 32 | `filter-store.ts::updateFilter` | `routes/filters/v1.ts:90` `filterV1Routes.put("/api/v1/filters/:id", async (c) =>` | arrow | 1 |
| 33 | `http.ts::requireUser` | `routes/accounts/blocks-mutes.ts:80` `accountBlockMuteRoutes.post("/api/v1/accounts/:id/mute", async (c) =>` | arrow | 1 |
| 34 | `cors.ts::apiCors` | `app.ts:48` `app.use("/api/*", apiCors());` | middleware | 1 |
| 35 | `account-moderation-store.ts::setAccountModeration` | `routes/admin/accounts.ts:413` `adminAccountRoutes.post("/api/v1/admin/accounts/:id/action", async (c) =>` | arrow | 1 |
| 36 | `announcement-admin-store.ts::deleteAnnouncementById` | `routes/admin/announcements.ts:192` `adminAnnouncementRoutes.delete("/api/v1/admin/announcements/:id", async ...` | arrow | 1 |
| 37 | `notification-policy-store.ts::dismissNotificationRequests` | `routes/notification-requests.ts:287` `notificationRequestRoutes.post("/api/v1/notifications/requests/:id/accep...` | arrow | 2 |
| 38 | `follow-store.ts::rejectFollowRequest` | `routes/follow-requests.ts:201` `followRequestRoutes.post("/api/v1/follow_requests/:id/reject", async (c) =>` | arrow | 1 |
| 39 | `routes/admin/custom-emojis.ts::fileFrom` | `routes/admin/custom-emojis.ts:54` `adminCustomEmojiRoutes.post("/api/v1/admin/custom_emojis", async (c) =>` | arrow | 1 |
| 40 | `routes/accounts/lookup.ts::push` | `routes/accounts/lookup.ts:102` `accountLookupRoutes.get("/api/v1/accounts/search", async (c) =>` | arrow | 1 |
| 41 | `http.ts::queryOffset` | `routes/admin/trends.ts:46` `adminTrendRoutes.get("/api/v1/admin/trends/tags", async (c) =>` | arrow | 1 |
| 42 | `registration-block-store.ts::deleteEmailDomainBlock` | `routes/admin/email-blocks.ts:215` `adminEmailBlockRoutes.delete("/api/v1/admin/email_domain_blocks/:id", as...` | arrow | 1 |
| 43 | `poll-store.ts::pollFromRow` | `routes/polls.ts:21` `pollRoutes.get("/api/v1/polls/:id", async (c) =>` | arrow | 3 |
| 44 | `account-store.ts::searchAccounts` | `routes/accounts/lookup.ts:102` `accountLookupRoutes.get("/api/v1/accounts/search", async (c) =>` | arrow | 1 |
| 45 | `announcement-admin-store.ts::setAnnouncementPublished` | `routes/admin/announcements.ts:230` `adminAnnouncementRoutes.post("/api/v1/admin/announcements/:id/publish", ...` | arrow | 2 |
| 46 | `routes/accounts/profile.ts::accountList` | `routes/accounts/profile.ts:149` `accountProfileRoutes.get("/api/v1/accounts/:id/followers", (c) => accoun...` | arrow | 1 |
| 47 | `custom-emoji-store.ts::findOne` | `routes/admin/custom-emojis.ts:54` `adminCustomEmojiRoutes.post("/api/v1/admin/custom_emojis", async (c) =>` | arrow | 2 |
| 48 | `status-store.ts::updateLocalNote` | `routes/statuses/edit.ts:325` `statusEditRoutes.put("/api/v1/statuses/:id", (c) => updateStatus(c));` | arrow | 2 |
| 49 | `status-idempotency-store.ts::expiredBefore` | `routes/statuses/create.ts:428` `statusCreateRoutes.post("/api/v1/statuses", async (c) =>` | arrow | 2 |
| 50 | `filter-store.ts::keywordsJson` | `routes/filters/v1.ts:47` `filterV1Routes.post("/api/v1/filters", async (c) =>` | arrow | 2 |

### web-ui: functions that ran but are called untested

<!-- derived-from #m1-test-reach-against-coverage -->

All 15, read from source:

| Cause | Ran, called untested | Functions |
|---|---|---|
| Under a call-valued module constant (`const StatusParser = mastodon.type("StatusPayloadApi").pipe(toStatus)`, `export const parseStatus = StatusParser`): the test's `parseStatus(..)` call names a node that does not exist (#239) | 10 | `toStatus`, `toOriginal`, `toAccountRef`, `toPreviewCard`, `toPoll`, `PreviewCard.fromApi`, `MediaAttachment.fromApi`, `toProfileFields`, `notificationFromPayload`, `Notification.favourite` |
| A shorthand member (`profileSnapshotSource = { fetchAccountProfile, .. }`) (#239) | 1 | `fetchAccountProfile` |
| A method passed as a value (`items.filter(ComposerMedia.isReady)`, `fromPromise(.., HttpError.fromUnknown)`) | 3 | `ComposerMedia.isReady`, `ComposerMedia.isUploading`, `HttpError.fromUnknown` |
| A factory called at module load (`mastodonPostJson = sendJson("POST")`) | 1 | `sendJson` |

### web-ui: functions called tested that did not run

<!-- derived-from #m1-test-reach-against-coverage -->

All 91, counted by script from each call site and the coverage of the
function it sits in: a call site inside a nested function (an event
handler, a callback) that never ran; a call site in a function that ran,
where the call did not happen (a branch); or a call site in a function that
never ran, whose own call sites then decide. The script's rows were read
against the call sites in source.

| Cause | Called tested, did not run |
|---|---|
| Every call site is inside an event handler or callback that never ran, directly or through callers that never ran (`ProfilePage`'s follow handler calls `followAccount`; the test renders the page and clicks nothing) | 54 |
| Registered in the route tree and never visited: 20 lazy pages (`export const ThreadPage = lazy(async () => import(..))`), 4 route components that `buildRouteTree` names, and `useStreamingTimeline` under `HomePage`; the router test does not navigate to them | 25 |
| A branch the test does not take (`{lightbox && <MediaLightbox ..>}`, `editingFilterId ? updateKeywordFilter(..) : createKeywordFilter(..)` in a save handler that ran) | 10 |
| Some call sites on an untaken branch, the others in handlers that never ran (`mapStatusResponse`, `LinkPreviewCard`) | 2 |

These are web-ui's lazy `await import()` routes. They make answers wrong in
the other direction from T3's: reaching through the lazy import to the
page would call more pages tested, not fewer.
