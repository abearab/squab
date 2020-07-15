mod context;
mod filter;
mod reader;
mod writer;

pub use self::{context::Context, filter::Filter, reader::Reader, writer::Writer};

use std::{collections::HashSet, convert::TryFrom, io};

use interval_tree::IntervalTree;
use noodles_bam as bam;
use noodles_gff as gff;
use noodles_sam::{self as sam, header::ReferenceSequences};

use crate::{CigarToIntervals, Entry, Features, PairPosition, RecordPairs, StrandSpecification};

pub fn count_single_end_records<I>(
    records: I,
    features: &Features,
    references: &ReferenceSequences,
    filter: &Filter,
    strand_specification: StrandSpecification,
) -> io::Result<Context>
where
    I: Iterator<Item = io::Result<bam::Record>>,
{
    let mut ctx = Context::default();

    for result in records {
        let record = result?;

        count_single_end_record(
            &mut ctx,
            features,
            references,
            filter,
            strand_specification,
            &record,
        )?;
    }

    Ok(ctx)
}

pub fn count_single_end_record(
    ctx: &mut Context,
    features: &Features,
    reference_sequences: &ReferenceSequences,
    filter: &Filter,
    strand_specification: StrandSpecification,
    record: &bam::Record,
) -> io::Result<()> {
    if filter.filter(ctx, record)? {
        return Ok(());
    }

    let cigar = record.cigar();
    let start = (record.position() + 1) as u64;
    let flags = record.flags();

    let reverse = match strand_specification {
        StrandSpecification::Reverse => true,
        _ => false,
    };

    let intervals = CigarToIntervals::new(&cigar, start, flags, reverse);

    let tree = match get_tree(
        ctx,
        features,
        reference_sequences,
        record.reference_sequence_id(),
    )? {
        Some(t) => t,
        None => return Ok(()),
    };

    let set = find(tree, intervals, strand_specification);

    update_intersections(ctx, set);

    Ok(())
}

pub fn count_paired_end_records<I>(
    records: I,
    features: &Features,
    reference_sequences: &ReferenceSequences,
    filter: &Filter,
    strand_specification: StrandSpecification,
) -> io::Result<(Context, RecordPairs<I>)>
where
    I: Iterator<Item = io::Result<bam::Record>>,
{
    let mut ctx = Context::default();

    let primary_only = !filter.with_secondary_records() && !filter.with_supplementary_records();
    let mut pairs = RecordPairs::new(records, primary_only);

    for pair in &mut pairs {
        let (r1, r2) = pair?;

        if filter.filter_pair(&mut ctx, &r1, &r2)? {
            continue;
        }

        let cigar = r1.cigar();
        let start = (r1.position() + 1) as u64;
        let f1 = r1.flags();

        let reverse = match strand_specification {
            StrandSpecification::Reverse => true,
            _ => false,
        };

        let intervals = CigarToIntervals::new(&cigar, start, f1, reverse);

        let tree = match get_tree(
            &mut ctx,
            features,
            reference_sequences,
            r1.reference_sequence_id(),
        )? {
            Some(t) => t,
            None => continue,
        };

        let mut set = find(tree, intervals, strand_specification);

        let cigar = r2.cigar();
        let start = (r2.position() + 1) as u64;
        let f2 = r2.flags();

        let reverse = match strand_specification {
            StrandSpecification::Reverse => false,
            _ => true,
        };

        let intervals = CigarToIntervals::new(&cigar, start, f2, reverse);

        let tree = match get_tree(
            &mut ctx,
            features,
            reference_sequences,
            r2.reference_sequence_id(),
        )? {
            Some(t) => t,
            None => continue,
        };

        let set2 = find(tree, intervals, strand_specification);

        set.extend(set2.into_iter());

        update_intersections(&mut ctx, set);
    }

    Ok((ctx, pairs))
}

pub fn count_paired_end_record_singletons<I>(
    records: I,
    features: &Features,
    reference_sequences: &ReferenceSequences,
    filter: &Filter,
    strand_specification: StrandSpecification,
) -> io::Result<Context>
where
    I: Iterator<Item = io::Result<bam::Record>>,
{
    let mut ctx = Context::default();

    for result in records {
        let record = result?;

        if filter.filter(&mut ctx, &record)? {
            continue;
        }

        let cigar = record.cigar();
        let start = (record.position() + 1) as u64;

        let reverse = match PairPosition::try_from(&record) {
            Ok(PairPosition::First) => false,
            Ok(PairPosition::Second) => true,
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "record is neither read 1 nor 2",
                ))
            }
        };

        let reverse = match strand_specification {
            StrandSpecification::Reverse => !reverse,
            _ => reverse,
        };

        let flags = record.flags();
        let intervals = CigarToIntervals::new(&cigar, start, flags, reverse);

        let tree = match get_tree(
            &mut ctx,
            features,
            reference_sequences,
            record.reference_sequence_id(),
        )? {
            Some(t) => t,
            None => continue,
        };

        let set = find(tree, intervals, strand_specification);

        update_intersections(&mut ctx, set);
    }

    Ok(ctx)
}

fn find(
    tree: &IntervalTree<u64, Entry>,
    intervals: CigarToIntervals,
    strand_specification: StrandSpecification,
) -> HashSet<String> {
    let mut set = HashSet::new();

    for (interval, is_reverse) in intervals {
        for entry in tree.find(interval.clone()) {
            let gene_name = &entry.get().0;
            let strand = &entry.get().1;

            match strand_specification {
                StrandSpecification::None => {
                    set.insert(gene_name.to_string());
                }
                StrandSpecification::Forward | StrandSpecification::Reverse => {
                    if (strand == &gff::record::Strand::Reverse && is_reverse)
                        || (strand == &gff::record::Strand::Forward && !is_reverse)
                    {
                        set.insert(gene_name.to_string());
                    }
                }
            }
        }
    }

    set
}

fn get_reference<'a>(
    reference_sequences: &'a ReferenceSequences,
    ref_id: i32,
) -> io::Result<&'a sam::header::ReferenceSequence> {
    if ref_id < 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected ref id >= 0, got {}", ref_id),
        ));
    }

    reference_sequences
        .get_index(ref_id as usize)
        .map(|(_, rs)| rs)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "expected ref id < {}, got {}",
                    reference_sequences.len(),
                    ref_id
                ),
            )
        })
}

fn update_intersections(ctx: &mut Context, intersections: HashSet<String>) {
    if intersections.is_empty() {
        ctx.no_feature += 1;
    } else if intersections.len() == 1 {
        for name in intersections {
            let count = ctx.counts.entry(name).or_insert(0);
            *count += 1;
        }
    } else if intersections.len() > 1 {
        ctx.ambiguous += 1;
    }
}

pub fn get_tree<'t>(
    ctx: &mut Context,
    features: &'t Features,
    reference_sequences: &ReferenceSequences,
    ref_id: i32,
) -> io::Result<Option<&'t IntervalTree<u64, Entry>>> {
    let reference = get_reference(reference_sequences, ref_id)?;
    let name = reference.name();

    match features.get(name) {
        Some(t) => Ok(Some(t)),
        None => {
            ctx.no_feature += 1;
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_reference_sequences() -> ReferenceSequences {
        vec![
            (
                String::from("chr1"),
                sam::header::ReferenceSequence::new(String::from("chr1"), 7),
            ),
            (
                String::from("chr2"),
                sam::header::ReferenceSequence::new(String::from("chr2"), 12),
            ),
            (
                String::from("chr3"),
                sam::header::ReferenceSequence::new(String::from("chr3"), 148),
            ),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn test_get_reference() {
        let reference_sequences = build_reference_sequences();

        let reference = get_reference(&reference_sequences, 1).unwrap();
        assert_eq!(reference.name(), "chr2");
        assert_eq!(reference.len(), 12);

        let reference = get_reference(&reference_sequences, -2);
        assert!(reference.is_err());

        let reference = get_reference(&reference_sequences, 5);
        assert!(reference.is_err());
    }
}
