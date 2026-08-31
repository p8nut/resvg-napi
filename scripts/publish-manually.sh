#!/usr/bin/env bash
#
# Publish a release by hand, with your own OTP.
#
# CI is the normal path: the `publish` job runs on a `v*` tag with a token that
# bypasses 2FA. This is the fallback for when that token cannot be used, and it
# encodes what went wrong the first time so it does not go wrong again:
#
#   - Platform packages first, the root last. An npm version is immutable, so a
#     failure part-way must not leave a root package whose optionalDependencies
#     do not exist. Orphan platform packages nothing references are harmless.
#   - The root is only attempted if every platform package verified. Thirteen
#     out of thirteen or nothing.
#   - Every result is checked against the registry, never against the CLI. On
#     2026-08-30 `npm unpublish` printed `- <name>` for fourteen packages while
#     the DELETE behind it returned 404 and nothing was removed; the next day
#     `npm publish` reported a 409 for a package the registry then accepted
#     forty minutes later. It is wrong in both directions.
#   - An OTP lasts about thirty seconds and a release takes longer, so EOTP is
#     expected mid-run and simply asks again rather than aborting.
#
# Usage:  npm login  &&  bash scripts/publish-manually.sh
#         bash scripts/publish-manually.sh --force   # skip the window check
set -uo pipefail
cd "$(dirname "$0")/.."

FORCE=0
[ "${1:-}" = "--force" ] && FORCE=1

say() { printf '%s\n' "$*"; }
die() { printf '\n!! %s\n' "$*" >&2; exit 1; }

# --- who, and what ----------------------------------------------------------
who=$(npm whoami 2>&1) || die "not logged in: $who   (run: npm login)"
version=$(node -p "require('./package.json').version")
root=$(node -p "require('./package.json').name")
mapfile -t dirs < <(ls -d npm/*/ | sed 's:/$::')

say "  account : $who"
say "  release : $root@$version"
say "  packages: ${#dirs[@]} platform + 1 root"

# --- is the window open? ----------------------------------------------------
if [ "$FORCE" = "0" ]; then
  say ""
  say "  checking the names are publishable..."
  node scripts/check-npm-window.mjs >/tmp/window.$$ 2>&1
  rc=$?
  # The contract is exit 0 = blocked, exit 1 *with the marker* = free. Anything
  # else is the check itself failing, and a broken check must not read as a
  # go-ahead: that is how a crashing script -- a missing file, in the first
  # version of this one -- announced an open window over thirteen tombstones.
  if [ "$rc" = "0" ]; then
    grep -E '^blocked' /tmp/window.$$ | sed 's/^/    /'
    rm -f /tmp/window.$$
    die "some names still carry a tombstone -- npm will refuse. Re-run when the
    daily 'npm window' job goes red, or pass --force to try anyway."
  fi
  if [ "$rc" != "1" ] || ! grep -q 'the npm names are free' /tmp/window.$$; then
    sed 's/^/    /' /tmp/window.$$
    rm -f /tmp/window.$$
    die "the window check did not run (exit $rc). Not guessing -- fix it, or
    pass --force if you are certain."
  fi
  rm -f /tmp/window.$$
  say "  window is open."
fi

# --- confirm ----------------------------------------------------------------
say ""
read -r -p "  publish $root@$version to npm? this cannot be undone [y/N] " yn
case "$yn" in [yY]*) ;; *) die "aborted";; esac

OTP=""
ask_otp() { read -r -p "  OTP: " OTP; }
ask_otp

# `+ name@version` is npm's success line. Believed only until the registry
# confirms it below.
publish_one() {
  local dir="$1" name="$2" out
  for attempt in 1 2 3; do
    out=$(npm publish "$dir" --access public --provenance --otp="$OTP" 2>&1)
    if printf '%s' "$out" | grep -q "^+ $name@"; then return 0; fi
    if printf '%s' "$out" | grep -q 'code EOTP'; then
      say "    OTP expired, need a fresh one"
      ask_otp
      continue
    fi
    printf '%s' "$out" | grep -E 'npm error' | head -3 | sed 's/^/      /'
    return 1
  done
  return 1
}

# The registry is the authority.
confirm_one() {
  local name="$1"
  local got
  got=$(curl -sS "https://registry.npmjs.org/${name//\//%2F}?t=$(date +%s%N)" \
        | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{
            try{const d=JSON.parse(s);process.stdout.write(Object.keys(d.versions||{}).join(","))}catch{}})')
  case ",$got," in *",$version,"*) return 0;; *) return 1;; esac
}

# --- platform packages ------------------------------------------------------
say ""
say "  platform packages (root comes last, and only if these all land)"
failed=()
for dir in "${dirs[@]}"; do
  name=$(node -p "require('./$dir/package.json').name")
  printf '    %-34s ' "$name"
  if publish_one "$dir" "$name" && confirm_one "$name"; then
    say "ok"
  else
    say "FAILED"
    failed+=("$name")
  fi
done

if [ "${#failed[@]}" -gt 0 ]; then
  say ""
  say "  ${#failed[@]} platform package(s) did not land:"
  printf '    %s\n' "${failed[@]}"
  die "the root package is NOT published. Nothing can install a broken release.
    Fix the cause, then re-run: the ones that landed will report
    'cannot publish over previously published version', which is expected --
    bump the version if you need to retry them too."
fi

# --- the root ---------------------------------------------------------------
say ""
printf '  root package %-21s ' "$root"
if publish_one "." "$root" && confirm_one "$root"; then
  say "ok"
  say ""
  say "  published: $root@$version, ${#dirs[@]} platform packages behind it"
  say "  verify:    npm install $root@$version"
else
  say "FAILED"
  die "the platform packages are published but the root is not. They are
    orphans nothing references, which is harmless. Re-run this script -- the
    platform ones will refuse as already published and the root will retry."
fi
