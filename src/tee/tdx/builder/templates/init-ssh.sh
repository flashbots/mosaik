# --- SSH access ---
mkdir -p /root/.ssh
cat >> /root/.ssh/authorized_keys <<'SSH_KEYS_EOF'
{{SSH_KEYS}}
SSH_KEYS_EOF
chmod 700 /root/.ssh
chmod 600 /root/.ssh/authorized_keys

# Start dropbear SSH server if available
if command -v dropbear >/dev/null 2>&1; then
    mkdir -p /etc/dropbear
    dropbear -R -E -B 2>/dev/null &
    echo "  [ssh] dropbear started"
fi
