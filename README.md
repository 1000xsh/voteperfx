## voteperfx - solana vote monitor
solana vote monitor with real-time tvc (timely vote credits) tracking.

## features
- real-time vote monitoring via yellowstone grpc
- tvc efficiency tracking and performance metrics
- interactive dashboard with latency visualization
- automatic poor performance detection and logging
- optimized for low resource usage

## setup
```bash
# clone and build
git clone https://github.com/1000xsh/voteperfx
cd voteperfx
cargo build --release

# configure
nano config.toml
# edit config.toml with your grpc endpoint and vote account
```
## usage

```bash
# interactive dashboard (default)
./target/release/voteperfx

# simple logging mode
./target/release/voteperfx --simple

# help
./target/release/voteperfx --help
```

## configuration

edit `config.toml` to set:
- `grpc_url`: your yellowstone grpc endpoint
- `vote_account`: validator vote account to monitor
- `performance_logging`: filters for logging poor performance events
