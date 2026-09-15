#!/bin/sh
set -eu

# Gardr supplies the sole allowlist. Resolve and pin it before denying all other
# egress so a CDN address cannot rotate after the firewall has admitted it.
domains_file=${3:?effective domains file is required}
[ -f "$domains_file" ] || { echo "missing effective domains file" >&2; exit 1; }

iptables -F
iptables -X
ipset destroy gardr-allowed 2>/dev/null || true
ipset create gardr-allowed hash:ip

while read -r resolver; do
    case "$resolver" in
        *[!0-9.]*|'') echo "unsupported DNS resolver: $resolver" >&2; exit 1 ;;
    esac
    iptables -A OUTPUT -d "$resolver" -p udp --dport 53 -j ACCEPT
    iptables -A OUTPUT -d "$resolver" -p tcp --dport 53 -j ACCEPT
    iptables -A INPUT -s "$resolver" -p udp --sport 53 -j ACCEPT
    iptables -A INPUT -s "$resolver" -p tcp --sport 53 -j ACCEPT
done <<EOF
$(awk '$1 == "nameserver" { print $2 }' /etc/resolv.conf)
EOF

while read -r domain; do
    [ -z "$domain" ] && continue
    ips=$(dig +short A "$domain")
    [ -n "$ips" ] || { echo "could not resolve $domain" >&2; exit 1; }
    pinned=''
    for ip in $ips; do
        case "$ip" in
            *[!0-9.]*|'') echo "invalid IPv4 address for $domain" >&2; exit 1 ;;
        esac
        ipset add gardr-allowed "$ip" -exist
        [ -n "$pinned" ] || pinned=$ip
    done
    printf '%s\t%s\n' "$pinned" "$domain" >> /etc/hosts
done < "$domains_file"

iptables -A INPUT -i lo -j ACCEPT
iptables -A OUTPUT -o lo -j ACCEPT
iptables -A INPUT -m state --state ESTABLISHED,RELATED -j ACCEPT
iptables -A OUTPUT -m state --state ESTABLISHED,RELATED -j ACCEPT
iptables -A OUTPUT -m set --match-set gardr-allowed dst -j ACCEPT
iptables -P INPUT DROP
iptables -P FORWARD DROP
iptables -P OUTPUT DROP
iptables -A OUTPUT -j REJECT --reject-with icmp-admin-prohibited
