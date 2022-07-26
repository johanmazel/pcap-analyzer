use pcap_parser::{Capture, PcapCapture};
use std::fs;
use std::fs::File;
use std::path::Path;

pub fn count_packet_in_trace(trace_file_path: &Path) -> u32 {
    if trace_file_path.exists() {
        let file = File::open(trace_file_path).unwrap();
        let file_size = file.metadata().unwrap().len();
        if file_size == 0 {
            0
        } else {
            let data = fs::read(trace_file_path).unwrap();
            let cap = PcapCapture::from_file(&data).unwrap();
            let mut count = 0;
            let mut iter = cap.iter();
            while iter.next().is_some() {
                count += 1;
            }
            count
        }
    } else {
        panic!("{:#?} does not exists!", trace_file_path)
    }
}
