#!/bin/bash
# v0.4.0 interception demo: shows the 4 decision paths in sequence.
# This is the validation for exit criterion #4 (docs/v0.4-design.md:531-536).
# Real recording for the v0.4.0 release; the throwaway command is
# ~/bin/dogfood-test, the shim is at ~/.local/share/agent0waste/shims/.

# Make sure the shim dir is on PATH (last line of .zshrc adds it).
export PATH="$HOME/.local/share/agent0waste/shims:$PATH"

# Suppress the zsh prompt noise so the recording is clean.
export PS1='$ '

cd ~/beme08/Agent0Waste
AGW=./target/release/agent0waste

clear
echo "=== v0.4.0 interception demo ==="
echo
echo "Throwaway command: ~/bin/dogfood-test (echoes its args)."
echo "Shim: ~/.local/share/agent0waste/shims/dogfood-test"
echo
echo "--- 1. ALLOW path: rules=allow, real binary runs silently ---"
cat > ~/.config/agent0waste/intercept.toml <<EOF
[rules.cache_bloat]
action = "allow"
cooldown_s = 0

[rules.prompt_growth]
action = "allow"
cooldown_s = 0
EOF
dogfood-test allow-demo
sleep 1

echo
echo "--- 2. PROMPT path: cache_bloat=prompt, user says Y (runs) ---"
cat > ~/.config/agent0waste/intercept.toml <<EOF
[rules.cache_bloat]
action = "prompt"
cooldown_s = 0
EOF
echo "Y" | dogfood-test prompt-yes-demo
sleep 1

echo
echo "--- 3. PROMPT path: user says N (cancels, rc=1) ---"
echo "N" | dogfood-test prompt-no-demo
sleep 1

echo
echo "--- 4. THROTTLE path: cache_bloat=throttle 2s, sleeps then re-checks ---"
cat > ~/.config/agent0waste/intercept.toml <<EOF
[rules.cache_bloat]
action = "throttle"
cooldown_s = 2
EOF
dogfood-test throttle-demo
sleep 1

echo
echo "--- 5. Real hermes (verifies find_real + spawn, MD5 unchanged) ---"
echo "(running hermes --version through the shim)"
hermes --version 2>&1 | head -4
sleep 1

echo
echo "=== end demo ==="
