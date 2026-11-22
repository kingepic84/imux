#!/bin/bash
set -u
cd "$(dirname "$0")/.." || exit 1
fail=0

if grep -rnE '\b(println|eprintln|print|eprint|dbg)!' src; then
  echo "FAIL stray print macros: log through crate::log instead"
  fail=1
else
  echo "ok   no print macros in src"
fi

if grep -rn 'set_hook\|install_panic_hook' src/main.rs >/dev/null; then
  echo "ok   panic hook installed in main"
else
  echo "FAIL panic hook missing in main"
  fail=1
fi

if grep -c 'guarded(' src/server/mod.rs | grep -qv '^0$'; then
  echo "ok   server event loop guards its work"
else
  echo "FAIL server work is not guarded against panics"
  fail=1
fi
exit $fail
