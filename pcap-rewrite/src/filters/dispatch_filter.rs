use std::io;
use std::net::IpAddr;
use std::path::Path;

use libpcap_tools::FiveTuple;
use pcap_parser::data::PacketData;
use pnet_packet::ethernet::{EtherType, EtherTypes};
use pnet_packet::ip::IpNextHeaderProtocol;
use pnet_packet::PrimitiveValues;

use crate::container::five_tuple_container::FiveTupleC;
use crate::container::ipaddr_container::IpAddrC;
use crate::container::ipaddr_proto_port_container::IpAddrProtoPortC;
use crate::filters::filter::FResult;
use crate::filters::filter::Filter;
use crate::filters::filter_utils;
use crate::filters::filtering_action::FilteringAction;
use crate::filters::filtering_key::FilteringKey;
use crate::filters::key_parser_ipv4;
use crate::filters::key_parser_ipv6;

pub struct DispatchFilter<C, D> {
    key_container: C,
    get_key_from_ipv4_l3_data: Box<dyn Fn(&[u8]) -> Result<D, String>>,
    get_key_from_ipv6_l3_data: Box<dyn Fn(&[u8]) -> Result<D, String>>,
    keep: Box<dyn Fn(&C, &D) -> Result<bool, String>>,
}

impl<C, D> DispatchFilter<C, D> {
    pub fn new(
        key_container: C,
        get_key_from_ipv4_l3_data: Box<dyn Fn(&[u8]) -> Result<D, String>>,
        get_key_from_ipv6_l3_data: Box<dyn Fn(&[u8]) -> Result<D, String>>,
        keep: Box<dyn Fn(&C, &D) -> Result<bool, String>>,
    ) -> Self {
        DispatchFilter {
            key_container,
            get_key_from_ipv4_l3_data,
            get_key_from_ipv6_l3_data,
            keep,
        }
    }

    pub fn keep<'j>(&self, packet_data: PacketData<'j>) -> FResult<PacketData<'j>, String> {
        let keep = match packet_data {
            PacketData::L2(data) => {
                if data.len() < 14 {
                    return FResult::Error("L2 data too small for ethernet".to_owned());
                }

                filter_utils::extract_test_callback_ethernet(
                    &self.key_container,
                    &self.get_key_from_ipv4_l3_data,
                    &self.get_key_from_ipv6_l3_data,
                    &self.keep,
                    data,
                )
            }
            PacketData::L3(l3_layer_value_u8, data) => {
                let ether_type = EtherType::new(l3_layer_value_u8 as u16);
                match ether_type {
                    EtherTypes::Ipv4 => filter_utils::extract_test_callback_ipv4(
                        &self.key_container,
                        &self.get_key_from_ipv4_l3_data,
                        &self.keep,
                        data,
                    ),
                    EtherTypes::Ipv6 => filter_utils::extract_test_callback_ipv6(
                        &self.key_container,
                        &self.get_key_from_ipv6_l3_data,
                        &self.keep,
                        data,
                    ),
                    _ => Err(format!(
                        "Unimplemented Ethertype in L3 {:?}/{:x}",
                        ether_type,
                        ether_type.to_primitive_values().0
                    )),
                }
            }
            PacketData::L4(_, _) => unimplemented!(),
            PacketData::Unsupported(_) => unimplemented!(),
        };
        match keep {
            Ok(b) => {
                if b {
                    FResult::Ok(packet_data)
                } else {
                    FResult::Drop
                }
            }
            Err(s) => FResult::Error(s),
        }
    }
}

impl<C, D> Filter for DispatchFilter<C, D> {
    fn filter<'i>(&self, i: PacketData<'i>) -> FResult<PacketData<'i>, String> {
        self.keep(i)
    }
}

pub struct DispatchFilterBuilder;

impl DispatchFilterBuilder {
    pub fn from_args(
        filtering_key: FilteringKey,
        filtering_action: FilteringAction,
        key_file_path: &str,
    ) -> Result<Box<dyn Filter>, io::Error> {
        match filtering_key {
            FilteringKey::SrcIpaddr => {
                let ipaddr_container = IpAddrC::of_file_path(Path::new(key_file_path))
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

                let keep: &dyn Fn(&IpAddrC, &IpAddr) -> Result<bool, String> =
                    match filtering_action {
                        FilteringAction::Keep => &|c: &IpAddrC, ipaddr| Ok(c.contains(ipaddr)),
                        FilteringAction::Drop => &|c: &IpAddrC, ipaddr| Ok(!c.contains(ipaddr)),
                    };

                Ok(Box::new(DispatchFilter::new(
                    ipaddr_container,
                    Box::new(key_parser_ipv4::parse_src_ipaddr),
                    Box::new(key_parser_ipv6::parse_src_ipaddr),
                    Box::new(keep),
                )))
            }
            FilteringKey::DstIpaddr => {
                let ipaddr_container = IpAddrC::of_file_path(Path::new(key_file_path))
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

                let keep: &dyn Fn(&IpAddrC, &IpAddr) -> Result<bool, String> =
                    match filtering_action {
                        FilteringAction::Keep => &|c: &IpAddrC, ipaddr| Ok(c.contains(ipaddr)),
                        FilteringAction::Drop => &|c: &IpAddrC, ipaddr| Ok(!c.contains(ipaddr)),
                    };

                Ok(Box::new(DispatchFilter::new(
                    ipaddr_container,
                    Box::new(key_parser_ipv4::parse_dst_ipaddr),
                    Box::new(key_parser_ipv6::parse_dst_ipaddr),
                    Box::new(keep),
                )))
            }
            FilteringKey::SrcDstIpaddr => {
                let ipaddr_container = IpAddrC::of_file_path(Path::new(key_file_path))
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

                let keep: &dyn Fn(&IpAddrC, &(IpAddr, IpAddr)) -> Result<bool, String> =
                    match filtering_action {
                        FilteringAction::Keep => &|c, ipaddr_tuple| {
                            Ok(c.contains(&ipaddr_tuple.0) || c.contains(&ipaddr_tuple.1))
                        },
                        FilteringAction::Drop => &|c, ipaddr_tuple| {
                            Ok(!c.contains(&ipaddr_tuple.0) && !c.contains(&ipaddr_tuple.1))
                        },
                    };

                Ok(Box::new(DispatchFilter::new(
                    ipaddr_container,
                    Box::new(key_parser_ipv4::parse_src_dst_ipaddr),
                    Box::new(key_parser_ipv6::parse_src_dst_ipaddr),
                    Box::new(keep),
                )))
            }
            FilteringKey::SrcIpaddrProtoDstPort => {
                let ipaddr_proto_port_container =
                    IpAddrProtoPortC::of_file_path(Path::new(key_file_path))
                        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

                let keep: &dyn Fn(
                    &IpAddrProtoPortC,
                    &(IpAddr, IpNextHeaderProtocol, u16),
                ) -> Result<bool, String> = match filtering_action {
                    FilteringAction::Keep => {
                        &|c, tuple| Ok(c.contains(&tuple.0, &tuple.1, tuple.2))
                    }
                    FilteringAction::Drop => {
                        &|c, tuple| Ok(!c.contains(&tuple.0, &tuple.1, tuple.2))
                    }
                };

                Ok(Box::new(DispatchFilter::new(
                    ipaddr_proto_port_container,
                    Box::new(key_parser_ipv4::parse_src_ipaddr_proto_dst_port),
                    Box::new(key_parser_ipv6::parse_src_ipaddr_proto_dst_port),
                    Box::new(keep),
                )))
            }
            FilteringKey::SrcDstIpaddrProtoSrcDstPort => {
                let five_tuple_container = FiveTupleC::of_file_path(Path::new(key_file_path))
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;

                let keep: &dyn Fn(&FiveTupleC, &FiveTuple) -> Result<bool, String> =
                    match filtering_action {
                        FilteringAction::Keep => &|c, five_tuple| Ok(c.contains(five_tuple)),
                        FilteringAction::Drop => &|c, five_tuple| Ok(!c.contains(five_tuple)),
                    };

                Ok(Box::new(DispatchFilter::new(
                    five_tuple_container,
                    Box::new(key_parser_ipv4::parse_five_tuple),
                    Box::new(key_parser_ipv6::parse_five_tuple),
                    Box::new(keep),
                )))
            }
        }
    }
}
