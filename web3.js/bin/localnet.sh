#!/usr/bin/env bash
set -e

channel=$(
  cd "$(dirname "$0")";
  node -p '
    let p = [
      "../../package.json",
      "../lib/node_modules/@solana/web3.js/package.json",
      "../@solana/web3.js/package.json",
      "../package.json"
    ].find(require("fs").existsSync);
    if (!p) throw new Error("Unable to locate package.json");
    require(p)["testnetDefaultChannel"]
  '
)

usage() {
  exitcode=0
  if [[ -n "$1" ]]; then
    exitcode=1
    echo "Error: $*"
  fi
  cat <<EOF
usage: $0 [update|up|down|logs] [command-specific options]

Operate a local testnet

 update   - Update the image from dockerhub.com
 up       - Start the cluster
 down     - Stop the cluster
 logs     - Display cluster logging

 logs-specific options:
   -f    - Follow log output

 update-specific options:
   <tag> - Optional Docker image tag to use

 up-specific options:
   <tag> - Optional Docker image tag to use
   -n    - Optional Docker network to join

   Default channel: $channel

 down-specific options:
   none

EOF
  exit $exitcode
}

[[ -n $1 ]] || usage
cmd="$1"
shift

docker --version || usage "It appears that docker is not installed"
case $cmd in
update)
  if [[ -n $1 ]]; then
    channel="$1"
  fi
  (
    set -x
    docker pull solanalabs/solana:"$channel"
  )
  ;;
up)
  while [[ -n $1 ]]; do
    if [[ $1 = -n ]]; then
      [[ -n $2 ]] || usage "Invalid $1 argument"
      network="$2"
      shift 2
    else
      channel=$1
      shift 1
    fi
  done

  (
    set -x
    RUST_LOG=${RUST_LOG:-solana=info}
    ARGS=(
      --detach
      --name solana-localnet
      --rm
      --publish 8899:8899
      --publish 8900:8900
      --publish 9900:9900
      --tty
      --ulimit "nofile=500000"
      --env "RUST_LOG=$RUST_LOG"
    )
    if [[ -n $network ]]; then
      ARGS+=(--network "$network")
    fi

    docker run "${ARGS[@]}" solanalabs/solana:"$channel"

    for _ in 1 2 3 4 5; do
      if curl \
          -X POST \
          -H "Content-Type: application/json" \
          -d '{"jsonrpc":"2.0","id":1, "method":"getTransactionCount"}' \
          http://localhost:8899; then
        break;
      fi
      sleep 1
    done
  )
  ;;
down)
  (
    set -x
    if [[ $(docker ps --filter "name=^/solana-localnet$" -q) ]]; then
      docker stop --time 0 solana-localnet
    fi
  )
  ;;
logs)
  follow=false
  if [[ -n $1 ]]; then
    if [[ $1 = "-f" ]]; then
      follow=true
    else
      usage "Unknown argument: $1"
    fi
  fi

  while $follow; do
    if [[ $(docker ps -q -f "name=^/solana-localnet$") ]]; then
      (
        set -x
        docker logs solana-localnet -f
      ) || true
    fi
    sleep 1
  done

  (
    set -x
    docker logs solana-localnet
  )
  ;;
*)
  usage "Unknown command: $cmd"
esac

exit 0
