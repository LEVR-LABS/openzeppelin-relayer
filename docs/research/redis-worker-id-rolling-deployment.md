# Redis worker IDs and rolling-deployment stale transactions

Research date: 2026-09-17

Scope: the checked-out LEVR fork at `b08a3b87`, OpenZeppelin Relayer `main`,
Apalis/`apalis-redis` 0.7.4, and AWS ECS/ELB documentation. Sources are primary
project or vendor sources.

## Conclusion

**Report this to OpenZeppelin Relayer upstream as a bug, after one final exact-title
and symptom search. Do not open a new Apalis bug for the same mechanism.** Apalis
already tracks the abandoned-job defect in issue #504 and merged PRs #507/#508.
More importantly, the Apalis maintainer states that two workers with one name are
unsupported and lead to undefined behavior because Redis cannot distinguish their
jobs.[1][2] Current OpenZeppelin `main` nevertheless gives every replica the same
fixed names, including `transaction_status_checker_evm`.[3] The OpenZeppelin issue
search performed for `apalis`, `worker id`, `orphaned`, and `graceful shutdown`
found no issue that describes this fixed-ID rolling-deployment collision.[4]

Graceful shutdown **reduces the probability** of this incident but **cannot remove
the race or provide a recovery guarantee**. A cleanly delivered SIGTERM can let the
old task finish and acknowledge work. However, ECS can send SIGKILL when
`stopTimeout` expires; processes can crash; hosts can fail; deployments overlap old
and new tasks by design; and OpenZeppelin's Apalis monitor currently waits only five
seconds. Unique per-process worker IDs are the required correctness fix. Graceful
shutdown and a suitably large ECS stop timeout are additional loss-reduction
controls, not substitutes.[3][5][6]

No inspected OpenZeppelin documentation gives rolling-deployment or ECS graceful-
shutdown instructions. It documents multi-instance mode and Docker operation, but
not unique Redis worker IDs, SIGTERM drain behavior, `stopTimeout`, deployment
overlap, or load-balancer deregistration.[7]

## Proven facts

### Apalis 0.7.4 behavior

- This fork resolves both `apalis` and `apalis-redis` to 0.7.4.[8]
- `apalis-redis` 0.7.4 defaults to a 30-second heartbeat and a five-minute orphan
  threshold. An in-flight Redis set is named with the worker ID, and `keep_alive`
  updates the consumer record for that same ID. Orphan recovery selects expired
  consumers and re-enqueues jobs from their corresponding in-flight sets.[9]
- On worker startup, 0.7.4 also calls `reenqueue_orphaned(..., Utc::now())`; this was
  introduced by PR #507 as a quick fix for #504.[1][9] It does not make duplicate
  IDs safe.
- In the PR discussion, an Apalis contributor identified both relevant cases:
  another live process with the same ID keeps the consumer alive, while immediate
  restart can occur before expiry. After a test showed startup recovery could steal
  a still-running peer's job, the Apalis maintainer stated that two workers with the
  same name “should not be done” and produce “undefined behavior”; the backend
  cannot distinguish them.[2]

Therefore, when an old ECS task owns an in-flight job and a replacement uses the
same ID, Redis represents both processes as one consumer. A heartbeat from the new
task can keep that shared consumer current, so age-based orphan recovery cannot
identify only the old owner's work.[2][9]

### OpenZeppelin behavior and documentation

- Current OpenZeppelin `main` constructs all ordinary Redis workers with fixed
  `WorkerBuilder::new(...)` names. The EVM status worker is exactly
  `transaction_status_checker_evm`; there is no task-, host-, process-, UUID-, or
  boot-specific suffix.[3]
- Its monitor listens for SIGINT, SIGTERM, and a programmatic shutdown signal and
  configures `shutdown_timeout(Duration::from_millis(5000))`.[3] The checked-out
  fork has the same worker naming and five-second monitor timeout, then joins worker
  handles with a broader 35-second bound during application shutdown.[10]
- OpenZeppelin's user docs describe Docker deployment and warn against direct public
  exposure. The README describes `DISTRIBUTED_MODE=true` for multi-instance
  scheduled-work locks. Neither source documents rolling replacement, worker-ID
  uniqueness, Apalis orphan semantics, SIGTERM draining, ECS `stopTimeout`, or ELB
  deregistration delay.[7]

### Exact AWS guarantees

- Rolling ECS deployments use `minimumHealthyPercent` and `maximumPercent` to bound
  task counts. With defaults of 100% and 200%, ECS may start replacements before it
  stops old tasks. These settings control capacity and health, not queue-worker
  identity or job ownership.[5]
- For a service replacement with a load balancer, ECS removes the task from the
  load balancer and waits for connection draining, then performs the equivalent of
  `docker stop`: SIGTERM, followed by SIGKILL after the stop timeout if the process
  has not exited. The documented default is 30 seconds.[5][6]
- ECS `healthCheckGracePeriodSeconds` only tells the scheduler to ignore unhealthy
  load-balancer, VPC Lattice, and container health checks after startup. Container
  health-check `startPeriod` similarly delays counting startup failures. Neither
  delays worker startup nor serializes Redis consumers.[5][6]
- ALB deregistration delay stops new load-balanced requests and allows existing HTTP
  requests/connections to complete; its default is 300 seconds. It says nothing
  about internally polled Redis jobs. If no active connection exists, deregistration
  can complete immediately.[11]

## Inference

1. **Why the production symptom can persist:** the fixed ID is both the Redis
   consumer identity and part of its in-flight-set key. During normal rolling
   overlap, the new task refreshes the same consumer record. An old task that dies
   before acknowledging leaves work under an identity that still appears alive.
2. **Why graceful shutdown is insufficient:** it only covers cooperative shutdowns
   whose handlers finish before both the Apalis five-second drain and the outer ECS
   termination budget. It cannot cover SIGKILL, crash, host loss, OOM, Redis/network
   interruption, or work longer than the budget. It also does not correct the
   ambiguous identity while old and new tasks coexist.
3. **Why ELB settings do not fix this:** deregistration drains inbound HTTP traffic,
   while the Redis worker polls independently. A task can stop receiving HTTP and
   still own or acquire queue jobs until application shutdown reaches the worker.

## Recommendations

1. **Correctness fix:** generate one stable-for-process, unique-on-every-start ID
   suffix and apply it to every Redis-backed Apalis worker name (for example,
   `<role>-<ECS task ID or UUID>`). Keep the role prefix for observability. Apply the
   same rule to dynamically named swap workers. Test two overlapping replicas and
   forced termination of one while it owns a long-running job.
2. **Operational hardening:** retain SIGTERM handling; stop queue intake promptly;
   drain in-flight handlers; and set ECS `stopTimeout` above the measured worst-case
   drain plus margin. Make the application's drain timeout fit inside that value.
   This lowers forced interruption but does not replace unique IDs.
3. **Do not tune deployment health or ALB deregistration as the primary fix.** They
   can shape overlap and HTTP draining, but ECS rolling deployment still permits
   overlap and none gives Redis job-ownership guarantees.
4. **Preserve retry/idempotency protections.** Unique IDs restore orphan detection;
   recovery is still at-least-once in failure cases, so a transaction handler must
   safely tolerate replay.

## Candidate upstream reports

### OpenZeppelin Relayer — open a bug

Target: <https://github.com/OpenZeppelin/openzeppelin-relayer/issues/new>

Suggested title: **Redis workers reuse Apalis WorkerId across replicas, preventing
safe orphan recovery during rolling deployments**

Include: OpenZeppelin revision/version, `apalis-redis` version, two overlapping
instances, fixed worker name, Redis keys/heartbeats, one killed owner, stale
Submitted transaction, links to Apalis #504 and PR #507 discussion, and a minimal
two-worker reproduction. Ask for unique per-process IDs plus a regression test and
deployment documentation. This is an OpenZeppelin integration defect because its
source chooses fixed IDs contrary to Apalis's stated constraint.[2][3]

Before filing, repeat a narrow GitHub search by the suggested title and exact fixed
worker name. The current broad search found no duplicate, but search results can
change.[4]

### Apalis — do not open a duplicate bug

The underlying abandoned-job and duplicate-ID behavior is already recorded in
#504 and the #507 discussion; #508 adds failure-path tests.[1][2][12] A focused
documentation issue or PR would be reasonable only if current Apalis docs still do
not state that Redis worker IDs must be unique per concurrently running process.
Reference the maintainer's existing statement instead of reopening the bug.

## Sources

1. Apalis issue #504 and merged PR #507: <https://github.com/apalis-dev/apalis/issues/504>, <https://github.com/apalis-dev/apalis/pull/507>
2. Duplicate-ID discussion and maintainer statement: <https://github.com/apalis-dev/apalis/pull/507#issuecomment-2653325453> (preceding comments contain the cases and reproduction)
3. OpenZeppelin `main` Redis worker source: <https://github.com/OpenZeppelin/openzeppelin-relayer/blob/main/src/queues/redis/worker.rs>
4. OpenZeppelin GitHub issue search: <https://github.com/OpenZeppelin/openzeppelin-relayer/issues?q=apalis+OR+%22worker+id%22+OR+orphaned+OR+%22graceful+shutdown%22>
5. AWS ECS service update/rolling-deployment parameters: <https://docs.aws.amazon.com/AmazonECS/latest/developerguide/update-service-parameters.html>
6. AWS ECS task lifecycle and stop-signal behavior: <https://docs.aws.amazon.com/AmazonECS/latest/developerguide/task-lifecycle-explanation.html>; task-definition health/start/stop parameters: <https://docs.aws.amazon.com/AmazonECS/latest/developerguide/task_definition_parameters.html>
7. OpenZeppelin Relayer docs and README: <https://docs.openzeppelin.com/relayer>, <https://github.com/OpenZeppelin/openzeppelin-relayer/blob/main/README.md>
8. Local dependency lock: [`Cargo.lock:1214-1262`](../../Cargo.lock#L1214-L1262)
9. Apalis Redis 0.7.4 storage source: <https://github.com/apalis-dev/apalis/blob/v0.7.4/packages/apalis-redis/src/storage.rs>; orphan Lua script: <https://github.com/apalis-dev/apalis/blob/v0.7.4/packages/apalis-redis/lua/reenqueue_orphaned_jobs.lua>
10. Local worker/shutdown paths: [`src/queues/redis/worker.rs:307-318`](../../src/queues/redis/worker.rs#L307-L318), [`src/queues/redis/worker.rs:376-586`](../../src/queues/redis/worker.rs#L376-L586), [`src/main.rs:291-321`](../../src/main.rs#L291-L321)
11. AWS ALB deregistration delay: <https://docs.aws.amazon.com/elasticloadbalancing/latest/application/edit-target-group-attributes.html#deregistration-delay>
12. Apalis PR #508: <https://github.com/apalis-dev/apalis/pull/508>
