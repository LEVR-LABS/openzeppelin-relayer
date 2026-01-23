# OpenZeppelin Relayer Deployment

## Branches

| Branch          | Environment | Config File              |
| --------------- | ----------- | ------------------------ |
| devops          | dev         | config.levr.json         |
| levr-v1-staging | staging     | config.staging.levr.json |
| levr-v1-main    | production  | config.prod.levr.json    |

## Required Environment Variables

- `API_KEY`: API key for relayer authentication
- `REDIS_URL`: Redis connection URL
- `AWS_ACCESS_KEY_ID`: AWS access key (for KMS signers)
- `AWS_SECRET_ACCESS_KEY`: AWS secret key (for KMS signers)

### Optional

- `LOG_LEVEL`: trace/debug/info/warn/error (default: info)
- `RATE_LIMIT_REQUESTS_PER_SECOND`: Rate limit (default: 100)
- `RATE_LIMIT_BURST_SIZE`: Burst size (default: 300)
- `METRICS_ENABLED`: Enable Prometheus metrics (default: false)

## Configuration File

The service requires a config file mounted at `/app/config`. The config file templates are in the `config/` directory:

- `config.levr.json` - dev environment
- `config.staging.levr.json` - staging environment
- `config.prod.levr.json` - production environment

**The config file must be created from the template** by substituting `MONAD_TESTNET_RPC_URL_1` with the actual RPC URL, **and made accessible to the container** (e.g., via volume mount).

Each config defines:

- **Relayers**: Transaction relayer instances with IDs, networks, and policies
- **Signers**: AWS KMS signers with key IDs per relayer group
- **Networks**: RPC URLs and chain configurations

Network configs are in `config/networks/` (monad.json, etc.)

## Relayer Groups

| Group            | Purpose                          |
| ---------------- | -------------------------------- |
| game_admin       | Game administration transactions |
| feed_provider    | Price feed transactions          |
| liquidator       | Liquidation transactions         |
| match_maker      | Order matching transactions      |
| position_handler | Position management transactions |

## AWS KMS Setup

Each signer requires an AWS KMS key. Key IDs are configured in the config JSON files under `signers[].config.key_id`.

## External Dependencies

- Redis
- AWS KMS keys

## Service Configuration

- **Port**: 8080
- **Metrics Port**: 8081 (if enabled)
- **Config Path**: `/app/config`

## Build

Use the provided Dockerfile.

## Deployment Checklist

- [ ] Config file created from template with RPC URLs substituted
- [ ] Config file accessible to container at `/app/config`
- [ ] Environment variables configured
- [ ] AWS KMS keys created and accessible
- [ ] Redis accessible
- [ ] Container/image deployed
- [ ] Service accessible on port 8080
- [ ] Relayer wallets funded
