use crate::default_plugin_builder;
use crate::packet_info::PacketInfo;
use crate::plugin::{Plugin, PluginResult, PLUGIN_FLOW_DEL, PLUGIN_L4};
use libpcap_tools::{Flow, FlowID, Packet};
use rusticata::prologue::*;
use std::collections::{HashMap, HashSet};

const PROBE_TCP: u32 = 0x0600_0000;
const PROBE_UDP: u32 = 0x1100_0000;

// This enum defines the order TCP probes will be applied
#[repr(u16)]
enum TcpProbeOrder {
    Dns,
    Tls,
    Ssh,
    Kerberos,
    OpenVpn,
}

// This enum defines the order UDP probes will be applied
#[repr(u16)]
#[allow(dead_code)]
enum UdpProbeOrder {
    Dhcp,
    Dns,
    Ikev2,
    Ikev2Natt,
    Kerberos,
    Ntp,
    OpenVpn,
    Radius,
    Snmpv1,
    Snmpv2c,
    Snmpv3,
}

// (filter, (name, probe))
type ProbeDef = (u32, (&'static str, ProbeL4));

#[derive(Default)]
pub struct Rusticata {
    builder_map: HashMap<&'static str, Box<dyn RBuilder>>,
    probes_l4: Vec<ProbeDef>,

    flow_probes: HashMap<FlowID, Vec<ProbeDef>>,
    flow_parsers: HashMap<FlowID, Box<dyn RParser>>,
    flow_bypass: HashSet<FlowID>,
}

default_plugin_builder!(Rusticata, RusticataBuilder);

macro_rules! add_parser {
    (tcp $name:expr, $pat:expr, $builder:expr, $bmap:ident, $probes:ident) => {
        let builder = $builder;
        if let Some(probe_fn) = builder.get_l4_probe() {
            $probes.push((PROBE_TCP | ($pat as u32), ($name, probe_fn)));
        }
        $bmap.insert($name, Box::new(builder) as Box<_>);
    };
    (udp $name:expr, $pat:expr, $builder:expr, $bmap:ident, $probes:ident) => {
        let builder = $builder;
        $bmap.insert($name, Box::new($builder) as Box<_>);
        if let Some(probe_fn) = builder.get_l4_probe() {
            $probes.push((PROBE_UDP | ($pat as u32), ($name, probe_fn)));
        }
    };
}

impl Plugin for Rusticata {
    fn name(&self) -> &'static str {
        "Rusticata"
    }
    fn plugin_type(&self) -> u16 {
        PLUGIN_L4 | PLUGIN_FLOW_DEL
    }

    fn pre_process(&mut self) {
        let mut builder_map: HashMap<&'static str, Box<dyn RBuilder>> = HashMap::new();
        let mut probes_l4: Vec<(u32, (&'static str, ProbeL4))> = Vec::new();

        // TCP
        add_parser!(tcp "dns_tcp", TcpProbeOrder::Dns, DnsTCPBuilder {}, builder_map, probes_l4);
        add_parser!(tcp "kerberos_tcp", TcpProbeOrder::Kerberos, KerberosTCPBuilder {}, builder_map, probes_l4);
        add_parser!(udp "openvpn_tcp", TcpProbeOrder::OpenVpn, OpenVPNTCPBuilder {}, builder_map, probes_l4);
        add_parser!(tcp "ssh", TcpProbeOrder::Ssh, SSHBuilder {}, builder_map, probes_l4);
        add_parser!(tcp "tls", TcpProbeOrder::Tls, TLSBuilder {}, builder_map, probes_l4);
        // UDP
        add_parser!(udp "dhcp", UdpProbeOrder::Dhcp, DHCPBuilder {}, builder_map, probes_l4);
        add_parser!(udp "dns_udp", UdpProbeOrder::Dns, DnsUDPBuilder {}, builder_map, probes_l4);
        add_parser!(udp "ikev2", UdpProbeOrder::Ikev2, IPsecBuilder {}, builder_map, probes_l4);
        add_parser!(udp "ikev2_natt", UdpProbeOrder::Ikev2Natt, IPsecNatTBuilder {}, builder_map, probes_l4);
        add_parser!(udp "kerberos_udp", UdpProbeOrder::Kerberos, KerberosUDPBuilder {}, builder_map, probes_l4);
        add_parser!(udp "ntp", UdpProbeOrder::Ntp, NTPBuilder {}, builder_map, probes_l4);
        add_parser!(udp "openvpn_udp", UdpProbeOrder::OpenVpn, OpenVPNUDPBuilder {}, builder_map, probes_l4);
        // add_parser!(udp "radius", UdpProbeOrder::Radius, RadiusBuilder {}, builder_map, probes_l4);
        add_parser!(udp "snmpv1", UdpProbeOrder::Snmpv1, SNMPv1Builder {}, builder_map, probes_l4);
        add_parser!(udp "snmpv2c", UdpProbeOrder::Snmpv2c, SNMPv2cBuilder {}, builder_map, probes_l4);
        add_parser!(udp "snmpv3", UdpProbeOrder::Snmpv3, SNMPv3Builder {}, builder_map, probes_l4);

        probes_l4.sort_unstable_by(|a, b| a.0.cmp(&b.0));

        self.builder_map = builder_map;
        self.probes_l4 = probes_l4;
    }

    fn handle_layer_transport<'s, 'i>(
        &'s mut self,
        _packet: &'s Packet,
        pinfo: &PacketInfo,
    ) -> PluginResult<'i> {
        let flow_id = match pinfo.flow {
            Some(f) => f.flow_id,
            None => {
                info!("No flow");
                return PluginResult::None;
            }
        };
        // did we already try all probes and fail? if yes return
        if self.flow_bypass.contains(&flow_id) {
            return PluginResult::None;
        }
        if let Some(d) = pinfo.l4_payload {
            if d.is_empty() {
                return PluginResult::None;
            }
            let parser = {
                // check if we already have a parser
                if let Some(parser) = self.flow_parsers.get_mut(&flow_id) {
                    parser
                } else {
                    // no parser, try to probe protocol
                    let l4_info = L4Info {
                        src_port: pinfo.five_tuple.src_port,
                        dst_port: pinfo.five_tuple.dst_port,
                        l4_proto: pinfo.l4_type,
                    };
                    let maybe_s = self.probe(d, flow_id, &l4_info);
                    if let Some(parser_name) = maybe_s {
                        debug!("Protocol recognized as {}", parser_name);
                        // warn!("Protocol recognized as {} (5t: {})", parser_name, pinfo.five_tuple);
                        if let Some(builder) = self.builder_map.get((&parser_name) as &str) {
                            self.flow_parsers.insert(flow_id, builder.build());
                            self.flow_parsers.get_mut(&flow_id).unwrap()
                        } else {
                            warn!("Could not build parser for proto {}", parser_name);
                            self.flow_bypass.insert(flow_id);
                            return PluginResult::None;
                        }
                    } else {
                        // proto not recognized
                        trace!("Parser not recognized");
                        return PluginResult::None;
                    }
                }
            };
            let direction = if pinfo.to_server {
                STREAM_TOSERVER
            } else {
                STREAM_TOCLIENT
            };
            let res = parser.parse(d, direction);
            if res == R_STATUS_FAIL {
                warn!(
                    "rusticata: parser failed (idx={}) (5t: {})",
                    pinfo.pcap_index, pinfo.five_tuple
                );
                // remove or disable parser for flow?
                let _ = self.flow_parsers.remove(&flow_id);
                // XXX add to bypass?
            }
        }
        PluginResult::None
    }

    fn flow_destroyed(&mut self, flow: &Flow) {
        self.flow_probes.remove(&flow.flow_id);
        self.flow_bypass.remove(&flow.flow_id);
    }

    fn post_process(&mut self) {
        for (flow_id, parser) in self.flow_parsers.iter() {
            info!("Flow: 0x{:x}", flow_id);
            for key in parser.keys() {
                info!("  [{}] => {:?}", key, parser.get(key));
            }
        }
    }
}

impl Rusticata {
    fn probe(&mut self, i: &[u8], flow_id: FlowID, l4_info: &L4Info) -> Option<String> {
        // check if we have a list of unsure probes
        // otherwise, iterate on full list
        let probes = match self.flow_probes.get(&flow_id) {
            Some(list) => list,
            None => &self.probes_l4,
        };
        let mut unsure_probes: Vec<ProbeDef> = Vec::new();
        let filter = (l4_info.l4_proto as u32) << 24;
        for (prio, (name, probe)) in probes.iter().filter(|(id, _)| id & filter != 0) {
            // debug!("trying probe {}", name);
            match probe(i, &l4_info) {
                ProbeResult::Certain | ProbeResult::Reverse => {
                    trace!("probe {} MATCHED", name);
                    let proto = (*name).to_string();
                    self.flow_probes.remove(&flow_id);
                    return Some(proto);
                }
                ProbeResult::Unsure => {
                    unsure_probes.push((*prio, (name, *probe)));
                }
                ProbeResult::NotForUs => (),
                ProbeResult::Fatal => {
                    warn!(
                        "Probe {} returned fatal error for flow ID 0x{:x}",
                        name, flow_id
                    );
                    // XXX disable probe if too many errors?
                }
            }
        }
        if unsure_probes.is_empty() {
            trace!("Adding flow to bypass");
            self.flow_probes.remove(&flow_id);
            self.flow_bypass.insert(flow_id);
        } else {
            self.flow_probes.insert(flow_id, unsure_probes);
        }
        None
    }
}
