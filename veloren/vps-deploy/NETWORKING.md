# Veloren VPS Networking Configuration

## Required Ports for Port Forwarding

For your VPS at IP address **52.247.50.106**, the following ports must be configured:

### Publicly Exposed Ports (Required for Players)

| Port | Protocol | Service | Purpose |
|------|----------|---------|---------|
| **14004** | TCP | Game Server | Main game server connection for players |
| **14005** | TCP | Game Server | Alternative game server connection |
| **14006** | UDP | Query Server | Server query/protocol for server lists |

### Private/Internal Ports (Do NOT expose publicly)

| Port | Protocol | Service | Purpose |
|------|----------|---------|---------|
| **19253** | TCP | Auth Service | Authentication server (keep private or use HTTPS reverse proxy) |
| **19254** | TCP | Marketplace | Marketplace service (keep private) |

## Firewall Configuration

### Using iptables (Linux)

```bash
# Allow game server ports
iptables -A INPUT -p tcp --dport 14004 -j ACCEPT
iptables -A INPUT -p tcp --dport 14005 -j ACCEPT
iptables -A INPUT -p udp --dport 14006 -j ACCEPT

# Keep auth ports private (only allow localhost or specific internal IPs)
iptables -A INPUT -p tcp --dport 19253 -s 127.0.0.1 -j ACCEPT
iptables -A INPUT -p tcp --dport 19254 -s 127.0.0.1 -j ACCEPT

# Allow SSH (adjust port as needed)
iptables -A INPUT -p tcp --dport 22 -j ACCEPT
```

### Using Windows Firewall

```powershell
# Allow game server ports
New-NetFirewallRule -DisplayName "Veloren Game Server 14004" -Direction Inbound -Protocol TCP -LocalPort 14004 -Action Allow
New-NetFirewallRule -DisplayName "Veloren Game Server 14005" -Direction Inbound -Protocol TCP -LocalPort 14005 -Action Allow
New-NetFirewallRule -DisplayName "Veloren Query Server 14006" -Direction Inbound -Protocol UDP -LocalPort 14006 -Action Allow

# Auth and marketplace (restrict to local if needed)
New-NetFirewallRule -DisplayName "Veloren Auth Server 19253" -Direction Inbound -Protocol TCP -LocalPort 19253 -Action Allow
New-NetFirewallRule -DisplayName "Veloren Marketplace 19254" -Direction Inbound -Protocol TCP -LocalPort 19254 -Action Allow
```

## VPS Provider Port Forwarding

### Common VPS Providers

#### AWS (Amazon Web Services)
1. Go to EC2 Security Groups
2. Add inbound rules for ports 14004-14006 (TCP/UDP)
3. Restrict 19253-19254 to specific IP ranges if needed

#### DigitalOcean
1. Go to Networking > Firewalls
2. Add rules for ports 14004-14006 (TCP/UDP)
3. Keep 19253-19254 restricted

#### Linode
1. Go to Networking > Firewalls
2. Configure inbound rules for game ports
3. Restrict internal services

#### Azure
1. Go to Network Security Groups
2. Add inbound rules for required ports
3. Configure service endpoints for internal communication

## Server Configuration

### Game Server Binding
The game server is configured to bind to `0.0.0.0:14004` (all interfaces) to accept connections from any IP.

**Configuration file:** `userdata/server/server_config/settings.ron`
```ron
gameserver_protocols: [
    Tcp(address: "0.0.0.0:14004"),
    Tcp(address: "[::]:14004"),
]
```

### Auth Server Binding
The auth server binds to `0.0.0.0:19253` but should be kept behind a reverse proxy for production use.

**Environment variable:** `BETA_AUTH_BIND=0.0.0.0:19253`

## Client Connection

Players connect to your server using:
- **IP:** `52.247.50.106`
- **Port:** `14004`
- **Example:** `52.247.50.106:14004`

## Testing Port Connectivity

### Test from Local Machine
```bash
# Test game server port
telnet 52.247.50.106 14004

# Test query server
nc -u 52.247.50.106 14006

# Test auth server (should fail if properly secured)
telnet 52.247.50.106 19253
```

### Test from VPS
```bash
# Check if services are listening
netstat -tulpn | grep -E '14004|14005|14006|19253|19254'

# Or on Windows
netstat -an | findstr "14004 14005 14006 19253 19254"
```

## Security Recommendations

1. **Keep auth ports private**: Only expose 19253 if behind HTTPS reverse proxy
2. **Use fail2ban**: Protect against brute force attacks on game ports
3. **Monitor connections**: Log and monitor unusual connection patterns
4. **Rate limiting**: Implement rate limiting on public ports
5. **Regular updates**: Keep your VPS OS and services updated

## Troubleshooting

### Players cannot connect
1. Verify ports 14004-14006 are open and accessible
2. Check firewall rules on VPS
3. Confirm VPS provider security group settings
4. Test connectivity using telnet/nc

### Auth server not working
1. Verify auth service is running on port 19253
2. Check auth.env configuration
3. Ensure game server can reach auth server
4. Review auth service logs

### Query server not showing in server list
1. Verify UDP port 14006 is open
2. Check query_address in settings.ron
3. Test UDP connectivity
4. Verify query service is running

## Reverse Proxy Configuration (Optional)

For production deployment, consider putting the auth server behind nginx or Apache:

### Nginx Example
```nginx
server {
    listen 443 ssl;
    server_name auth.veloren.net;

    ssl_certificate /path/to/cert.pem;
    ssl_certificate_key /path/to/key.pem;

    location / {
        proxy_pass http://127.0.0.1:19253;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }
}
```

This allows secure HTTPS access to the auth service while keeping the backend port private.
