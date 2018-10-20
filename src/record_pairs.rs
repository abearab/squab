use std::collections::HashMap;
use std::collections::hash_map::Drain;
use std::io::{self, Read};

use noodles::formats::bam::{self, ByteRecord, Flag};

#[derive(Debug, Eq, Hash, PartialEq)]
pub enum PairPosition {
    First,
    Second,
}

impl PairPosition {
    pub fn mate(&self) -> PairPosition {
        match *self {
            PairPosition::First => PairPosition::Second,
            PairPosition::Second => PairPosition::First,
        }
    }
}

impl<'a> From<&'a ByteRecord> for PairPosition {
    fn from(record: &ByteRecord) -> PairPosition {
        let flag = Flag::new(record.flag());
        PairPosition::from(flag)
    }
}

impl From<Flag> for PairPosition {
    fn from(flag: Flag) -> PairPosition {
        if flag.is_read_1() {
            PairPosition::First
        } else if flag.is_read_2() {
            PairPosition::Second
        } else {
            panic!("unknown pair position");
        }
    }
}

#[cfg(test)]
mod pair_position_tests {
    use noodles::formats::bam::Flag;

    use super::PairPosition;

    #[test]
    fn test_mate() {
        assert_eq!(PairPosition::First.mate(), PairPosition::Second);
        assert_eq!(PairPosition::Second.mate(), PairPosition::First);
    }

    #[test]
    fn test_from_flag() {
        let flag = Flag::new(0x41);
        assert_eq!(PairPosition::from(flag), PairPosition::First);

        let flag = Flag::new(0x81);
        assert_eq!(PairPosition::from(flag), PairPosition::Second);
    }

    #[test]
    #[should_panic]
    fn test_from_flag_with_invalid_flag() {
        let flag = Flag::new(0x01);
        PairPosition::from(flag);
    }
}

type RecordKey = (Vec<u8>, PairPosition, i32, i32, i32, i32, i32);

pub struct RecordPairs<R: Read> {
    reader: bam::Reader<R>,
    record: ByteRecord,
    buf: HashMap<RecordKey, ByteRecord>,
}

impl<R: Read> RecordPairs<R> {
    pub fn new(reader: bam::Reader<R>) -> RecordPairs<R> {
        RecordPairs {
            reader,
            record: ByteRecord::new(),
            buf: HashMap::new(),
        }
    }

    fn next_pair(&mut self) -> Option<io::Result<(ByteRecord, ByteRecord)>> {
        loop {
            match self.reader.read_byte_record(&mut self.record) {
                Ok(0) => {
                    if !self.buf.is_empty() {
                        warn!("{} records are singletons", self.buf.len());
                    }

                    return None;
                },
                Ok(_) => {},
                Err(e) => return Some(Err(e)),
            }

            let mate_key = mate_key(&self.record);

            if let Some(mate) = self.buf.remove(&mate_key) {
                return match mate_key.1 {
                    PairPosition::First => {
                        Some(Ok((mate, self.record.clone())))
                    },
                    PairPosition::Second => {
                        Some(Ok((self.record.clone(), mate)))
                    },
                };
            }

            let key = key(&self.record);

            self.buf.insert(key, self.record.clone());
        }
    }

    pub fn singletons(&mut self) -> Singletons {
        Singletons { drain: self.buf.drain() }
    }
}

impl<R: Read> Iterator for RecordPairs<R> {
    type Item = io::Result<(ByteRecord, ByteRecord)>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_pair()
    }
}

fn key(record: &ByteRecord) -> RecordKey {
    (
        record.read_name().to_vec(),
        PairPosition::from(record),
        record.ref_id(),
        record.pos(),
        record.next_ref_id(),
        record.next_pos(),
        record.tlen(),
    )
}

fn mate_key(record: &ByteRecord) -> RecordKey {
    (
        record.read_name().to_vec(),
        PairPosition::from(record).mate(),
        record.next_ref_id(),
        record.next_pos(),
        record.ref_id(),
        record.pos(),
        -record.tlen(),
    )
}

pub struct Singletons<'a> {
    drain: Drain<'a, RecordKey, ByteRecord>,
}

impl<'a> Iterator for Singletons<'a> {
    type Item = ByteRecord;

    fn next(&mut self) -> Option<Self::Item> {
        self.drain.next().map(|(_, r)| r)
    }
}
