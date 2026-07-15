use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

use mdns_sd::{Receiver, ScopedIp, ServiceDaemon, ServiceEvent, ServiceInfo};

use crate::net::discovery::{DiscoveryEndpoint, public_key_fingerprint};
use crate::net::protocol::{MDNS_SERVICE_TYPE, NodeId, PROTOCOL_VERSION, TransportHint};

pub struct MdnsDiscovery {
    daemon: ServiceDaemon,
    receiver: Receiver<ServiceEvent>,
    local_node_id: NodeId,
    host_name: String,
    instance_name: String,
    ip: Ipv4Addr,
    session_port: u16,
    public_key: [u8; 32],
}

impl MdnsDiscovery {
    pub fn new(
        local_node_id: NodeId,
        display_name: String,
        session_port: u16,
        public_key: [u8; 32],
    ) -> Result<Self, anyhow::Error> {
        let daemon = ServiceDaemon::new()?;
        let receiver = daemon.browse(MDNS_SERVICE_TYPE)?;

        let ip = local_ipv4().unwrap_or(Ipv4Addr::LOCALHOST);
        let host_name = format!("vocalcalc-{}.local.", short_node_id(local_node_id));
        let instance_name = format!("vc-{}", short_node_id(local_node_id));
        let fingerprint = public_key_fingerprint(&public_key);
        let service = build_service_info(
            local_node_id,
            &display_name,
            session_port,
            &host_name,
            &instance_name,
            ip,
            fingerprint,
        )?;
        daemon.register(service)?;
        log::info!(
            "mDNS registered {} on {}:{}",
            MDNS_SERVICE_TYPE,
            ip,
            session_port
        );

        Ok(Self {
            daemon,
            receiver,
            local_node_id,
            host_name,
            instance_name,
            ip,
            session_port,
            public_key,
        })
    }

    pub async fn recv_endpoint(&self) -> Result<DiscoveryEndpoint, anyhow::Error> {
        loop {
            match self.receiver.recv_async().await? {
                ServiceEvent::ServiceResolved(service) => {
                    let node_id = match service.get_property_val_str("node_id") {
                        Some(value) => match value.parse::<NodeId>() {
                            Ok(id) => id,
                            Err(e) => {
                                log::debug!("mDNS resolved service has invalid node_id: {}", e);
                                continue;
                            }
                        },
                        None => continue,
                    };
                    if node_id == self.local_node_id {
                        continue;
                    }

                    let display_name = service
                        .get_property_val_str("name")
                        .map(str::to_string)
                        .unwrap_or_else(|| service.get_fullname().to_string());
                    let port = service.get_port();
                    if port == 0 {
                        continue;
                    }

                    let ip = choose_address(service.get_addresses())
                        .or_else(|| {
                            service
                                .get_addresses_v4()
                                .into_iter()
                                .next()
                                .map(IpAddr::V4)
                        })
                        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
                    let fingerprint = service.get_property_val_str("pkfp").map(str::to_string);

                    return Ok(DiscoveryEndpoint {
                        node_id,
                        display_name,
                        address: SocketAddr::new(ip, port),
                        session_port: port,
                        transport_hint: TransportHint::Mdns,
                        public_key_fingerprint: fingerprint,
                    });
                }
                ServiceEvent::ServiceRemoved(_, fullname) => {
                    log::debug!("mDNS service removed: {}", fullname);
                }
                other => {
                    log::trace!("mDNS event: {:?}", other);
                }
            }
        }
    }

    pub fn update_display_name(&self, display_name: &str) -> Result<(), anyhow::Error> {
        let service = build_service_info(
            self.local_node_id,
            display_name,
            self.session_port,
            &self.host_name,
            &self.instance_name,
            self.ip,
            public_key_fingerprint(&self.public_key),
        )?;
        self.daemon.register(service)?;
        Ok(())
    }
}

fn build_service_info(
    local_node_id: NodeId,
    display_name: &str,
    session_port: u16,
    host_name: &str,
    instance_name: &str,
    ip: Ipv4Addr,
    fingerprint: String,
) -> Result<ServiceInfo, anyhow::Error> {
    let props = [
        ("node_id", local_node_id.to_string()),
        ("proto", PROTOCOL_VERSION.to_string()),
        ("name", display_name.to_string()),
        ("cap_control", "1".to_string()),
        ("cap_execute", "1".to_string()),
        ("pkfp", fingerprint),
    ];

    Ok(ServiceInfo::new(
        MDNS_SERVICE_TYPE,
        instance_name,
        host_name,
        ip.to_string(),
        session_port,
        &props[..],
    )?)
}

fn choose_address(addrs: &std::collections::HashSet<ScopedIp>) -> Option<IpAddr> {
    addrs
        .iter()
        .find(|ip| ip.is_ipv4() && !ip.is_loopback())
        .map(ScopedIp::to_ip_addr)
        .or_else(|| addrs.iter().next().map(ScopedIp::to_ip_addr))
}

fn local_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) => Some(ip),
        IpAddr::V6(_) => None,
    }
}

fn short_node_id(node_id: NodeId) -> String {
    node_id.as_simple().to_string().chars().take(12).collect()
}
