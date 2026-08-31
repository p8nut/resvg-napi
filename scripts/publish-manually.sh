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
PROBE=0
ASSUME_YES=0
OTP_ARG=""
for a in "$@"; do
  case "$a" in
    --force) FORCE=1 ;;
    # A write test on every name, costing nothing. Publishes a throwaway
    # 0.0.0-probe.<timestamp> under the `probe` dist-tag, from a temporary
    # manifest holding no files -- so npm answers the only question that
    # matters, "would you accept a write here", without the release version
    # being spent on the packages that say yes. Implies --force: testing the
    # window is the point.
    --probe) PROBE=1; FORCE=1 ;;
    --yes|-y) ASSUME_YES=1 ;;
    --otp=*) OTP_ARG="${a#--otp=}" ;;
    *) printf 'unknown flag: %s\n' "$a" >&2; exit 2 ;;
  esac
done

say() { printf '%s\n' "$*"; }
die() { printf '\n!! %s\n' "$*" >&2; exit 1; }

# --- who, and what ----------------------------------------------------------
who=$(npm whoami 2>&1) || die "not logged in: $who   (run: npm login)"
version=$(node -p "require('./package.json').version")
root=$(node -p "require('./package.json').name")
mapfile -t dirs < <(ls -d npm/*/ | sed 's:/$::')

if [ "$PROBE" = "1" ]; then
  version="0.0.0-probe.$(date +%s)"
  say "  account : $who"
  say "  MODE    : probe -- publishing $version, tagged 'probe', never 'latest'"
  say "            the real release version is not touched"
else
  say "  account : $who"
  say "  release : $root@$version"
fi
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
    total=$(grep -cE '^(blocked|free|published)' /tmp/window.$$)
    stuck=$(grep -cE '^blocked' /tmp/window.$$)
    say ""
    say "  $stuck of $total names still carry a tombstone:"
    grep -E '^blocked' /tmp/window.$$ | awk '{print "      " $2}'
    ready=$((total - stuck))
    [ "$ready" -gt 0 ] && {
      say "  the other $ready are fine:"
      grep -vE '^blocked' /tmp/window.$$ | grep -E '^(free|published)' \
        | awk '{printf "      %-34s (%s)\n", $2, $1}'
    }
    rm -f /tmp/window.$$
    die "npm will refuse these. Re-run when the daily 'npm window' job goes red,
    or pass --force to try anyway."
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
if [ "$ASSUME_YES" = "0" ]; then
  read -r -p "  publish $root@$version to npm? this cannot be undone [y/N] " yn
  case "$yn" in [yY]*) ;; *) die "aborted";; esac
fi

# A token with the 2FA bypass -- the one CI holds -- needs no code, and there is
# nobody at a keyboard to type one there anyway. An interactive session does,
# because the account is `auth-and-writes`.
OTP="$OTP_ARG"
HAVE_TOKEN=0
if [ -n "${NODE_AUTH_TOKEN:-}" ]; then
  HAVE_TOKEN=1
  # npm only reads NODE_AUTH_TOKEN through an .npmrc that mentions it, which
  # `setup-node` writes in CI and nothing writes on a laptop -- where the stale
  # token in ~/.npmrc wins instead, and every write comes back EOTP. Point npm
  # at a config of our own for the length of this run.
  NPMRC=$(mktemp)
  printf '//registry.npmjs.org/:_authToken=${NODE_AUTH_TOKEN}\n' > "$NPMRC"
  export npm_config_userconfig="$NPMRC"
  trap 'rm -f "$NPMRC"; [ -n "${PROBE_DIR:-}" ] && rm -rf "$PROBE_DIR"' EXIT
  who=$(npm whoami 2>&1) || die "NODE_AUTH_TOKEN did not authenticate: $who"
  say "  token  : authenticates as $who, 2FA bypassed"
fi
ask_otp() {
  if [ "$HAVE_TOKEN" = "1" ]; then return 0; fi
  if [ ! -t 0 ]; then die "npm wants an OTP and there is no terminal to ask on.
    Pass --otp=CODE, or set NODE_AUTH_TOKEN to a token that bypasses 2FA."; fi
  read -r -p "  OTP: " OTP
}
# No code is asked for up front. A token that bypasses 2FA needs none, and
# whether the one in ~/.npmrc is such a token is not knowable without trying:
# `npm whoami` succeeds either way. So the first write is attempted bare, and
# `publish_one` asks only if npm answers EOTP.

# `+ name@version` is npm's success line. Believed only until the registry
# confirms it below.
PROBE_DIR=""
[ "$PROBE" = "1" ] && PROBE_DIR=$(mktemp -d)
trap '[ -n "$PROBE_DIR" ] && rm -rf "$PROBE_DIR"' EXIT

publish_one() {
  local dir="$1" name="$2" out
  if [ "$PROBE" = "1" ]; then
    # A manifest with nothing in it: no binary is uploaded, and no provenance
    # is claimed for something that is not a build.
    dir="$PROBE_DIR/$name"
    mkdir -p "$dir"
    printf '{"name":"%s","version":"%s","description":"write probe"}\n' \
      "$name" "$version" > "$dir/package.json"
  fi
  for attempt in 1 2 3; do
    if [ "$PROBE" = "1" ]; then
      if [ -n "$OTP" ]; then
        out=$(npm publish "$dir" --access public --tag probe --otp="$OTP" 2>&1)
      else
        out=$(npm publish "$dir" --access public --tag probe 2>&1)
      fi
    else
      if [ -n "$OTP" ]; then
        out=$(npm publish "$dir" --access public --provenance --otp="$OTP" 2>&1)
      else
        out=$(npm publish "$dir" --access public --provenance 2>&1)
      fi
    fi
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

if [ "$PROBE" = "1" ]; then
  say ""
  printf '  root package %-21s ' "$root"
  if publish_one "." "$root" && confirm_one "$root"; then say "ok"; else say "REFUSE"; failed+=("$root"); fi
  say ""
  say "  ${#failed[@]} of $(( ${#dirs[@]} + 1 )) names refused the write."
  [ "${#failed[@]}" -gt 0 ] && printf '    %s\n' "${failed[@]}"
  say ""
  say "  nothing of value was published: $version is a throwaway under the"
  say "  'probe' tag. The release version is untouched."
  exit 0
fi

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
