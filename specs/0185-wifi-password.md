---
status: complete
---

# 0185 — Change the Wi-Fi password

## Purpose

Let a signed-in user change the Starlink router's Wi-Fi password from
Settings → Router. Electron only rewrites the passphrase as part of a subnet
move (`buildSubnetNetworks`). This change uses the same authenticated cloud
write with the subnet left alone. The router's LAN API refuses config writes
to anonymous clients, so the change requires a signed-in account.

## Requirements

- **SPEC-WIFIPW-001** — A Wi-Fi password write reads the router's current
  `wifiConfig` and sends back its networks:
  - the router-assigned `bssid` and `ifaceName` are dropped;
  - every WPA2, WPA3, and WPA2/WPA3 auth block that carries a password gets
    the new one, and RADIUS credentials are left alone;
  - the subnet is unchanged, and `applyNetworks` is set.

  A router that reports no networks is refused.

- **SPEC-WIFIPW-002** — A passphrase must be 8 to 63 characters, the same
  rule as the subnet move. The Subnet write shares this network rewrite.
- **SPEC-WIFIPW-003** — Settings → Router has new password and confirm fields
  with a save button:
  - they are enabled only while the account is signed in and no other router
    write is in flight;
  - a mismatch or a bad length is refused before anything is sent;
  - a confirmation warns that every device drops off and must rejoin with the
    new password;
  - the write is sent once and never retried, and the fields clear once the
    router accepts it.

## Acceptance criteria

- Cloud tests cover the request body and the refusals. GTK tests cover the
  form's gating and its refusals.
- Hardware: changing the password through a signed-in account takes effect,
  and devices rejoin with the new password.
