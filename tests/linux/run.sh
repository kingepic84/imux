#!/bin/bash
# Build and test imux on Linux in a container (Colima or any Docker). From the project directory:
#   tests/linux/run.sh up           build the image, start the container, copy the sources in
#   tests/linux/run.sh sync         copy the sources again (after edits)
#   tests/linux/run.sh unit [ARGS]  cargo test [ARGS]
#   tests/linux/run.sh clippy       cargo clippy --all-targets -- -D warnings
#   tests/linux/run.sh shell T...   build the debug binary, run tests/T.sh ... with tests/shims first on PATH
#   tests/linux/run.sh suite        build, then tests/run_all.sh (about 35 minutes or more)
#   tests/linux/run.sh sh [CMD]     a shell (or CMD) in the container
#   tests/linux/run.sh down         stop and remove the container; `down --all` also removes the volumes
# The sources are copied, not mounted, and the build output lives in a volume: a bind mount is slow
# and would mix macOS and Linux build output in target/.
set -eu
cd "$(dirname "$0")/../.."
NAME=imux-linux
IMAGE=imux-linux
run() { docker exec -i -e CARGO_TERM_COLOR=never "$NAME" bash -c "$*"; }

case "${1:-}" in
  up)
    docker build -t "$IMAGE" tests/linux
    docker rm -f "$NAME" >/dev/null 2>&1 || true
    docker run -d --name "$NAME" -v imux-target:/work/target -v imux-cargo:/home/dev/.cargo "$IMAGE" sleep infinity >/dev/null
    "$0" sync
    ;;
  sync)
    docker exec -u root "$NAME" chown -R dev:dev /work/target /home/dev/.cargo
    tar --no-xattrs --no-mac-metadata --exclude=./target --exclude=./.git --exclude=./fuzz/target --exclude=.DS_Store -cf - . | docker exec -i "$NAME" bash -c 'cd /work && tar -xf -'
    ;;
  unit)
    shift
    run "cd /work && cargo test $*"
    ;;
  clippy)
    run "cd /work && cargo clippy --all-targets -- -D warnings"
    ;;
  shell)
    shift
    run "cd /work && cargo build 2>&1 | tail -2 && for t in $*; do echo \"== \$t\"; PATH=/work/tests/shims:\$PATH bash tests/\$t.sh 2>&1; done"
    ;;
  suite)
    run "cd /work && cargo build 2>&1 | tail -2 && PATH=/work/tests/shims:\$PATH bash tests/run_all.sh"
    ;;
  sh)
    shift
    if [ $# -eq 0 ]; then exec docker exec -it "$NAME" bash -l; else run "$*"; fi
    ;;
  down)
    docker rm -f "$NAME" >/dev/null 2>&1 || true
    if [ "${2:-}" = "--all" ]; then
      docker volume rm imux-target imux-cargo >/dev/null 2>&1 || true
      docker rmi "$IMAGE" >/dev/null 2>&1 || true
    fi
    ;;
  *)
    sed -n 2,12p "$0"
    exit 1
    ;;
esac
