# OpenZeppelin Relayer v1.6.0 horizontal scaling

Research date: 2026-07-27

Scope: upstream OpenZeppelin Relayer v1.6.0 and the checked-out LEVR fork at commit
`144d7cb`. Sources are limited to official OpenZeppelin documentation/source and
this fork's source, configuration, and Git history.

## Conclusion

**Yes, this v1.6.0 fork can run more than one ECS task, provided every task uses
the same Redis repository/queue namespace and enables distributed mode.** The
upstream v1.6.0 release contains an explicit three-instance horizontal-scaling
example with shared Redis; its environment sets `DISTRIBUTED_MODE=true`, and all
three replicas set `RESET_STORAGE_ON_START=false`.[1][2][3]

The observed duplicate-network startup crash is not evidence that ordinary
distributed operation is unsupported. It is consistent with two instances
concurrently executing destructive config bootstrap against the same Redis DB
without effective distributed bootstrap coordination: reset deletes the shared
repositories and then recreates plugins, signers, notifications, networks,
relayers, and the API key in sequence; repository `create` methods reject an ID
that another process has already inserted.[4][5][6] With
`DISTRIBUTED_MODE=true`, v1.6.0 serializes that config-processing path, so the
specific concurrent insert race should not occur. However, leaving reset enabled
is still unsafe operationally: each newly started task that later obtains the
lock deliberately resets the shared state again.[4]

For a normal ECS service with desired count greater than one, set
`RESET_STORAGE_ON_START=false` on **every** service task. If file configuration
must replace Redis state, perform that destructive reconciliation once, with the
ECS service stopped or scaled to zero, through a one-off bootstrap job using the
same image/config/secrets/Redis namespace. Ensure the job succeeds, then start the
service replicas with reset disabled. A dedicated migration/bootstrap command
would be cleaner, but v1.6.0 has no separate command in the inspected startup
path: reset is implemented as part of normal process startup.[4][7]

## Recommended ECS configuration

Use the same values on every replica:

```text
REPOSITORY_STORAGE_TYPE=redis
REDIS_URL=<same primary Redis endpoint and logical DB, currently DB 8>
REDIS_KEY_PREFIX=<same stable, environment-specific prefix>
STORAGE_ENCRYPTION_KEY=<same key>
DISTRIBUTED_MODE=true
RESET_STORAGE_ON_START=false
QUEUE_BACKEND=redis                 # or one consistently configured shared backend
```

Operational requirements:

- Put all replicas behind the ECS load balancer and use liveness/readiness health
  checks. OpenZeppelin's reference topology load-balances three stateless HTTP
  instances over shared Redis.[1]
- All replicas for one logical deployment must use the same Redis logical DB and
  `REDIS_KEY_PREFIX`. Repository lock keys are derived from that prefix, and Redis
  queue namespaces are derived from the same environment value. Different DBs or
  prefixes create independent state, queues, and lock domains rather than one
  coordinated cluster.[8][9]
- Give each environment a distinct DB and/or prefix. Redis DB 8 is only a namespace
  boundary; there is no DB-8-specific behavior in the relayer. Two tasks using DB
  8 and the same prefix intentionally share state and therefore expose an unsafe
  startup reset to both tasks.[8][9]
- Use one writable primary endpoint for `REDIS_URL`. If `REDIS_READER_URL` is used,
  account for replica lag when sizing/operating the cluster; writes and distributed
  locks use the primary connection. OpenZeppelin also directs operators to divide
  the Redis connection budget across instance count.[10][11]
- Size Redis connection limits for the aggregate replica count. Each Redis-queue
  task creates eight dedicated queue connection managers in addition to repository
  pools.[9][10]
- Use Redis persistence/backups, authentication, private networking, TLS where the
  image is built with a Redis TLS feature, and a common storage-encryption key.
  These are official production storage recommendations, not supplied by
  `DISTRIBUTED_MODE` itself.[10]
- Keep the same config file and secrets across tasks. Config processing and
  initialization are coordinated by shared markers/locks; inconsistent task
  configuration would make whichever bootstrap wins authoritative and can make
  state validation misleading.[4][12]
- Do not treat distributed mode as exactly-once execution. Locks have finite TTLs,
  no renewal, and initialization deliberately degrades to uncoordinated work after
  Redis errors or prolonged timeout. Queue/backend delivery semantics still apply.
  Monitor Redis availability and keep protected operations below their lock TTLs.[8][12]

### One-off reset procedure

1. Stop or scale the ECS service to zero so no task serves or processes shared
   state while it is deleted.
2. Back up Redis if rollback is required.
3. Run exactly one temporary ECS task with the production image, mounted config,
   identical Redis DB/prefix/encryption key and secrets,
   `REPOSITORY_STORAGE_TYPE=redis`, `DISTRIBUTED_MODE=true`, and
   `RESET_STORAGE_ON_START=true`.
4. Wait for startup to complete successfully. The process sets an in-progress
   marker, drops repository entries and transaction counters, reloads config, then
   records completion.[4]
5. Stop that task and deploy the long-running ECS service at the desired count with
   `RESET_STORAGE_ON_START=false`.

This should be considered a destructive bootstrap, not an online rolling
migration. Reset deletes transaction records and nonce counters as well as config
entities.[4] It does **not** call a queue purge in the reset block, so old queued
jobs may outlive deleted repository state; quiescing producers/workers and deciding
how to drain or purge queues is required before a production reset.[4][9]

## Evidence

### Upstream support for multiple instances

- The v1.6.0 package identifies itself as version `1.6.0`.[13] LEVR's branch merged
  upstream tag `v1.6.0` (`554f15a`) at `47d5ab6`. The files changed from the tag to
  current HEAD are LEVR workflow/config/network/Postman and one gas-limit constant;
  none of the bootstrap, Redis, queue, or distributed-lock implementation differs.
  Therefore the analyzed scaling behavior is upstream v1.6.0 behavior, not an
  unreviewed LEVR modification.[14]
- OpenZeppelin ships `examples/horizontal-scaling` as a three-instance deployment
  using one Redis service for configuration, transaction state, job queues, and
  nonce coordination.[1] Its `.env.example` enables distributed mode.[2] Each of
  the three service definitions uses Redis repository storage and explicitly
  disables reset.[3]
- The root README says to use `DISTRIBUTED_MODE=true` for multi-instance deployments
  so scheduled workers use Redis locks and avoid duplicate execution.[15] Current
  official configuration docs state the same for multiple instances.[11]
- Official storage docs classify Redis as suitable for production,
  multi-instance, and scalable deployments; in-memory storage cannot be shared
  across instances.[10]

### What `DISTRIBUTED_MODE` actually does

`DISTRIBUTED_MODE` defaults to false and accepts case-insensitive `true` or `1`.
Its source documentation describes Redis locks for multi-instance coordination;
the flag is read dynamically from the environment.[16]

In v1.6.0 it protects these observed paths:

- **Config bootstrap:** with Redis storage and connection information available,
  `process_config_file` takes a global `{prefix}:lock:config_processing` lock,
  waits on an explicit completion marker when another instance holds it, and
  attempts takeover after timeout. Lock acquisition failure is fatal for this
  path rather than silently running the reset concurrently.[4]
- **Relayer initialization:** a global `{prefix}:lock:relayer_init_global` lock and
  five-minute per-relayer/global completion timestamps suppress redundant rolling
  restart work.[12]
- **Cron and cleanup work:** each cron handler obtains a prefix-scoped Redis lock;
  a task skips the tick if another instance owns it or Redis lock acquisition
  fails.[17] Transaction/system cleanup handlers also have explicit distributed
  locking.[18][19]
- **Nonce-health gap repair:** obtains a per-relayer Redis lock and skips if another
  task owns it.[20]

The flag is therefore broader than queue sharing but is not a cluster-membership
system or global exactly-once guarantee. The lock implementation is Redis
`SET key value NX EX ttl`, with a unique owner token and compare-and-delete Lua
release. Its own source warns that another instance can enter if work exceeds the
TTL and recommends a TTL above worst-case runtime or lock renewal; this
implementation does not renew locks.[8]

Relayer initialization has explicit availability-over-exclusivity fallbacks: on
lock errors it initializes without coordination, and after two approximately
130-second waits it may initialize despite a still-held lock. The source calls
duplicate side effects an accepted cost in that last-resort case.[12] This limits
the claim to safe normal operation with healthy Redis, not strict exactly-once
behavior under all failures.

### Shared queues, counters, and locking

- The default Redis queue backend uses Apalis `RedisStorage`. Every task constructs
  the same eight namespace names when `REDIS_KEY_PREFIX` is equal, giving all
  workers access to the same request, submission, status, notification, swap, and
  health queues.[9]
- The EVM transaction counter uses Redis `INCR`, an atomic allocation operation,
  rather than a process-local counter.[21]
- Repository and handler coordination uses the writable primary pool. Distributed
  lock keys include the common repository prefix.[8][17]

These mechanisms are why multiple consumers can participate in one pipeline. They
also explain why all replicas must point at one namespace; merely setting
`DISTRIBUTED_MODE=true` while using different DBs/prefixes coordinates nothing.

### Bootstrap and reset semantics

Normal startup is ordered: parse config, initialize repositories/queues, process
the config file, initialize relayers, and only then start background workers and
the HTTP service.[7]

For Redis with reset disabled, config is intended to load only on first startup;
later starts use Redis state. Official docs say API changes persist and
`RESET_STORAGE_ON_START=true` forces file config to override Redis.[10] Current
v1.6.0 source strengthens this with validation and an explicit completion marker:
empty state is bootstrapped, complete state is skipped/backfilled, and incomplete
bootstrap-managed state is an error.[4]

With reset enabled, the lock holder **always** executes reset, regardless of an
existing completion marker. It drops, in order, all relayers, transactions,
signers, notifications, networks, plugins, API keys, and transaction counters,
then recreates config entities in order.[4] The environment parser defaults reset
to false and only enables it when the value lowercases exactly to `true`.[16]

Consequences for simultaneous startup:

- **Without effective distributed mode:** both tasks can pass independent
  existence checks and interleave delete/create phases. Network and relayer
  repositories explicitly return constraint violations for pre-existing IDs;
  network creation's read-before-write is not one atomic conditional insert.
  This directly supports a duplicate-network crash as a bootstrap race.[5][6]
- **With effective distributed mode:** the v1.6.0 config lock serializes the two
  tasks, preventing concurrent config insertion. But reset semantics at lines
  572-575 mean task B still resets/reloads after task A completes. Thus a
  duplicate-network error under purported distributed mode should trigger checks
  that both tasks actually had `DISTRIBUTED_MODE=true`, identical Redis DB/prefix,
  Redis repository storage, and successful primary Redis lock access. It does not
  make always-on reset acceptable.[4]

The distinction matters: horizontal scaling is the steady-state feature;
`RESET_STORAGE_ON_START` is an opt-in destructive bootstrap operation. The
official horizontal-scaling example combines scaling with reset disabled.[1][3]

## Limitations and residual risks

- The source has no fencing token or lock refresh. A config or initialization
  operation lasting beyond the fixed 120-second bootstrap lock TTL can overlap a
  takeover.[8][22] A large LEVR config or slow KMS/RPC initialization should be
  timed before relying on this bound.
- Relayer initialization intentionally permits duplicate execution on Redis errors
  and extreme timeout; initialization side effects must remain tolerable.[12]
- Redis/Apalis queue consumption is distributed, but this review does not claim
  exactly-once message delivery. Handlers and transaction state must tolerate
  retries. OpenZeppelin's current docs similarly state that SQS Standard delivery
  may duplicate and relies on idempotent handlers.[11]
- Rate limits and worker concurrency are per process. Increasing ECS desired count
  multiplies aggregate API/worker concurrency and Redis/RPC/KMS pressure; tune the
  values and downstream quotas accordingly.[9][10]
- A rolling deploy with reset disabled does not reapply changed file config to an
  already initialized Redis namespace. Use API-based updates where appropriate or
  schedule the controlled one-off reconciliation above.[10]

## Sources

1. OpenZeppelin v1.6.0 horizontal-scaling example README: <https://github.com/OpenZeppelin/openzeppelin-relayer/blob/v1.6.0/examples/horizontal-scaling/README.md#L1-L59>
2. OpenZeppelin v1.6.0 horizontal-scaling environment: <https://github.com/OpenZeppelin/openzeppelin-relayer/blob/v1.6.0/examples/horizontal-scaling/.env.example#L1-L2>
3. OpenZeppelin v1.6.0 three-replica Compose configuration: <https://github.com/OpenZeppelin/openzeppelin-relayer/blob/v1.6.0/examples/horizontal-scaling/docker-compose.yaml#L31-L227>
4. Config coordination/reset implementation: [`src/bootstrap/config_processor.rs:390-768`](../../src/bootstrap/config_processor.rs#L390-L768), upstream <https://github.com/OpenZeppelin/openzeppelin-relayer/blob/v1.6.0/src/bootstrap/config_processor.rs#L390-L768>
5. Network duplicate constraint: [`src/repositories/network/network_redis.rs:253-289`](../../src/repositories/network/network_redis.rs#L253-L289), upstream <https://github.com/OpenZeppelin/openzeppelin-relayer/blob/v1.6.0/src/repositories/network/network_redis.rs#L253-L289>
6. Relayer duplicate constraint: [`src/repositories/relayer/relayer_redis.rs:137-168`](../../src/repositories/relayer/relayer_redis.rs#L137-L168), upstream <https://github.com/OpenZeppelin/openzeppelin-relayer/blob/v1.6.0/src/repositories/relayer/relayer_redis.rs#L137-L168>
7. Startup order: [`src/main.rs:65-124`](../../src/main.rs#L65-L124), upstream <https://github.com/OpenZeppelin/openzeppelin-relayer/blob/v1.6.0/src/main.rs#L65-L124>
8. Redis lock semantics and TTL warning: [`src/utils/redis.rs:236-438`](../../src/utils/redis.rs#L236-L438), upstream <https://github.com/OpenZeppelin/openzeppelin-relayer/blob/v1.6.0/src/utils/redis.rs#L236-L438>
9. Redis queue connections/namespaces: [`src/queues/redis/queue.rs:27-56`](../../src/queues/redis/queue.rs#L27-L56), [`src/queues/redis/queue.rs:161-278`](../../src/queues/redis/queue.rs#L161-L278), upstream <https://github.com/OpenZeppelin/openzeppelin-relayer/blob/v1.6.0/src/queues/redis/queue.rs#L161-L278>
10. Official storage documentation: <https://docs.openzeppelin.com/relayer/configuration/storage> (development docs matching v1.6.0 source features); archived stable storage semantics: <https://docs.openzeppelin.com/relayer/1.5.x/configuration/storage>
11. Official configuration documentation (`DISTRIBUTED_MODE`, queue backend, connection settings): <https://docs.openzeppelin.com/relayer/configuration>
12. Distributed relayer initialization and fallbacks: [`src/bootstrap/initialize_relayers.rs:1-46`](../../src/bootstrap/initialize_relayers.rs#L1-L46), [`src/bootstrap/initialize_relayers.rs:93-388`](../../src/bootstrap/initialize_relayers.rs#L93-L388), upstream <https://github.com/OpenZeppelin/openzeppelin-relayer/blob/v1.6.0/src/bootstrap/initialize_relayers.rs#L93-L388>
13. Version declaration: [`Cargo.toml:1-5`](../../Cargo.toml#L1-L5), upstream <https://github.com/OpenZeppelin/openzeppelin-relayer/blob/v1.6.0/Cargo.toml#L1-L5>
14. LEVR history: local commits `47d5ab6` (merge tag `v1.6.0`) and `554f15a` (upstream release); upstream release <https://github.com/OpenZeppelin/openzeppelin-relayer/releases/tag/v1.6.0>
15. Upstream multi-instance README guidance: [`README.md:376-386`](../../README.md#L376-L386), <https://github.com/OpenZeppelin/openzeppelin-relayer/blob/v1.6.0/README.md#L376-L386>
16. Flag parsing: [`src/config/server_config.rs:538-543`](../../src/config/server_config.rs#L538-L543), [`src/config/server_config.rs:621-632`](../../src/config/server_config.rs#L621-L632), upstream <https://github.com/OpenZeppelin/openzeppelin-relayer/blob/v1.6.0/src/config/server_config.rs#L538-L543>
17. Cron locking: [`src/queues/cron.rs:257-361`](../../src/queues/cron.rs#L257-L361), upstream <https://github.com/OpenZeppelin/openzeppelin-relayer/blob/v1.6.0/src/queues/cron.rs#L257-L361>
18. Transaction-cleanup locking: [`src/jobs/handlers/transaction_cleanup_handler.rs:102-155`](../../src/jobs/handlers/transaction_cleanup_handler.rs#L102-L155), upstream <https://github.com/OpenZeppelin/openzeppelin-relayer/blob/v1.6.0/src/jobs/handlers/transaction_cleanup_handler.rs#L102-L155>
19. System-cleanup locking: [`src/jobs/handlers/system_cleanup_handler.rs:109-172`](../../src/jobs/handlers/system_cleanup_handler.rs#L109-L172), upstream <https://github.com/OpenZeppelin/openzeppelin-relayer/blob/v1.6.0/src/jobs/handlers/system_cleanup_handler.rs#L109-L172>
20. Nonce-health locking: [`src/domain/relayer/evm/nonce.rs:390-469`](../../src/domain/relayer/evm/nonce.rs#L390-L469), upstream <https://github.com/OpenZeppelin/openzeppelin-relayer/blob/v1.6.0/src/domain/relayer/evm/nonce.rs#L390-L469>
21. Atomic transaction counter: [`src/repositories/transaction_counter/transaction_counter_redis.rs:110-126`](../../src/repositories/transaction_counter/transaction_counter_redis.rs#L110-L126), upstream <https://github.com/OpenZeppelin/openzeppelin-relayer/blob/v1.6.0/src/repositories/transaction_counter/transaction_counter_redis.rs#L110-L126>
22. Bootstrap timing constants: [`src/utils/redis.rs:12-24`](../../src/utils/redis.rs#L12-L24), upstream <https://github.com/OpenZeppelin/openzeppelin-relayer/blob/v1.6.0/src/utils/redis.rs#L12-L24>
