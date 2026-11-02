//! Linux default-gateway discovery for the router's default LAN endpoint.

use std::net::Ipv4Addr;

use crate::client::ROUTER_LAN_ADDRESS;

/// Parse `/proc/net/route` and select the active default route with the lowest
/// metric.
pub fn parse_proc_net_route(contents: &str) -> Option<Ipv4Addr> {
    contents
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            fields.next()?;
            let destination = u32::from_str_radix(fields.next()?, 16).ok()?;
            let gateway = u32::from_str_radix(fields.next()?, 16).ok()?;
            let flags = u16::from_str_radix(fields.next()?, 16).ok()?;
            fields.next()?;
            fields.next()?;
            let metric = fields.next()?.parse::<u32>().ok()?;
            let gateway = Ipv4Addr::from(gateway.to_le_bytes());

            (destination == 0 && flags & 0x3 == 0x3 && !gateway.is_unspecified())
                .then_some((metric, gateway))
        })
        .min_by_key(|(metric, _)| *metric)
        .map(|(_, gateway)| gateway)
}

/// Return the default router address from route-table contents, or the
/// historical address when there is no usable default route.
pub fn router_address_from_routes(contents: &str) -> Ipv4Addr {
    parse_proc_net_route(contents).unwrap_or_else(|| {
        ROUTER_LAN_ADDRESS
            .parse()
            .expect("router fallback is an IP")
    })
}

/// Discover the host's default gateway from Linux's IPv4 routing table.
pub fn default_router_address() -> Ipv4Addr {
    std::fs::read_to_string("/proc/net/route")
        .map(|routes| router_address_from_routes(&routes))
        .unwrap_or_else(|_| {
            ROUTER_LAN_ADDRESS
                .parse()
                .expect("router fallback is an IP")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC-CTRL-007: use the active default route with the lowest metric, and
    // fall back to the historical address if no usable route is present.
    #[test]
    fn selects_best_default_route_and_falls_back_when_absent() {
        let routes = "Iface Destination Gateway Flags RefCnt Use Metric Mask MTU Window IRTT\n\
                      wlp1s0 00000000 0100000A 0003 0 0 600 00000000 0 0 0\n\
                      eth0   00000000 0102A8C0 0003 0 0 100 00000000 0 0 0\n";
        assert_eq!(
            parse_proc_net_route(routes),
            Some(Ipv4Addr::new(192, 168, 2, 1))
        );
        assert_eq!(
            router_address_from_routes("Iface Destination Gateway Flags RefCnt Use Metric Mask\n"),
            Ipv4Addr::new(192, 168, 1, 1)
        );
    }

    #[test]
    fn ignores_malformed_down_and_non_default_routes() {
        let routes = "Iface Destination Gateway Flags RefCnt Use Metric Mask MTU Window IRTT\n\
                      wlp1s0 00000000 0100000A 0002 0 0 1 00000000 0 0 0\n\
                      wlp1s0 00000001 0100000A 0003 0 0 1 00000000 0 0 0\n\
                      wlp1s0 malformed 0100000A 0003 0 0 1 00000000 0 0 0\n";
        assert_eq!(parse_proc_net_route(routes), None);
    }
}
