mod pair_position;

pub use self::pair_position::PairPosition;

use std::{
    collections::{hash_map::Drain, HashMap},
    convert::TryFrom,
    io,
};

use log::warn;
use noodles_bam as bam;

type RecordKey = (
    Vec<u8>,
    PairPosition,
    Option<i32>,
    Option<i32>,
    Option<i32>,
    Option<i32>,
    i32,
);

pub struct RecordPairs<I> {
    records: I,
    buf: HashMap<RecordKey, bam::Record>,
    primary_only: bool,
}

impl<I> RecordPairs<I>
where
    I: Iterator<Item = io::Result<bam::Record>>,
{
    pub fn new(records: I, primary_only: bool) -> RecordPairs<I> {
        RecordPairs {
            records,
            buf: HashMap::new(),
            primary_only,
        }
    }

    fn next_pair(&mut self) -> Option<io::Result<(bam::Record, bam::Record)>> {
        loop {
            let record = match self.records.next() {
                Some(result) => match result {
                    Ok(r) => r,
                    Err(e) => return Some(Err(e)),
                },
                None => {
                    if !self.buf.is_empty() {
                        warn!("{} records are singletons", self.buf.len());
                    }

                    return None;
                }
            };

            if self.primary_only && is_not_primary(&record) {
                continue;
            }

            let mate_key = match mate_key(&record) {
                Ok(k) => k,
                Err(e) => return Some(Err(e)),
            };

            if let Some(mate) = self.buf.remove(&mate_key) {
                return match mate_key.1 {
                    PairPosition::First => Some(Ok((mate, record))),
                    PairPosition::Second => Some(Ok((record, mate))),
                };
            }

            let key = match key(&record) {
                Ok(k) => k,
                Err(e) => return Some(Err(e)),
            };

            self.buf.insert(key, record.clone());
        }
    }

    pub fn singletons(&mut self) -> Singletons {
        Singletons {
            drain: self.buf.drain(),
        }
    }
}

impl<I> Iterator for RecordPairs<I>
where
    I: Iterator<Item = io::Result<bam::Record>>,
{
    type Item = io::Result<(bam::Record, bam::Record)>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_pair()
    }
}

fn is_not_primary(record: &bam::Record) -> bool {
    let flags = record.flags();
    flags.is_secondary() || flags.is_supplementary()
}

fn key(record: &bam::Record) -> io::Result<RecordKey> {
    Ok((
        record
            .read_name()
            .map(|s| s.to_bytes().to_vec())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
        PairPosition::try_from(record).unwrap(),
        record.reference_sequence_id().map(i32::from),
        record.position().map(i32::from),
        record.mate_reference_sequence_id().map(i32::from),
        record.mate_position().map(i32::from),
        record.template_length(),
    ))
}

fn mate_key(record: &bam::Record) -> io::Result<RecordKey> {
    Ok((
        record
            .read_name()
            .map(|s| s.to_bytes().to_vec())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
        PairPosition::try_from(record).map(|p| p.mate()).unwrap(),
        record.mate_reference_sequence_id().map(i32::from),
        record.mate_position().map(i32::from),
        record.reference_sequence_id().map(i32::from),
        record.position().map(i32::from),
        -record.template_length(),
    ))
}

pub struct Singletons<'a> {
    drain: Drain<'a, RecordKey, bam::Record>,
}

impl<'a> Iterator for Singletons<'a> {
    type Item = bam::Record;

    fn next(&mut self) -> Option<Self::Item> {
        self.drain.next().map(|(_, r)| r)
    }
}
