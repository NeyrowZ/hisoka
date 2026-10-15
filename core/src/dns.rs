use crate::{Matcher, TTL_SECS};
use std::net::IpAddr;

const TYPE_A: u16 = 1;
const TYPE_AAAA: u16 = 28;
const TYPE_ANY: u16 = 255;
const CLASS_IN: u16 = 1;

const RCODE_OK: u8 = 0;
const RCODE_FORMERR: u8 = 1;
const RCODE_NXDOMAIN: u8 = 3;
const RCODE_NOTIMP: u8 = 4;
const RCODE_REFUSED: u8 = 5;

pub struct Reply {
    /// Bytes to send back to the client.
    pub packet: Vec<u8>,
    /// One-line description, for debug logging.
    pub summary: String,
}

struct Question {
    name: String,
    qtype: u16,
    qclass: u16,
    /// Offset just after the question section.
    end: usize,
}

fn parse_question(q: &[u8]) -> Option<Question> {
    let mut pos = 12;
    let mut name = String::new();
    loop {
        let len = *q.get(pos)? as usize;
        pos += 1;
        if len == 0 {
            break;
        }
        // Compression pointers (and reserved bits) never appear in a question.
        if len & 0xC0 != 0 {
            return None;
        }
        let label = q.get(pos..pos + len)?;
        pos += len;
        if !name.is_empty() {
            name.push('.');
        }
        for &b in label {
            name.push(b.to_ascii_lowercase() as char);
        }
        if name.len() > 253 {
            return None;
        }
    }
    let qtype = u16::from_be_bytes([*q.get(pos)?, *q.get(pos + 1)?]);
    let qclass = u16::from_be_bytes([*q.get(pos + 2)?, *q.get(pos + 3)?]);
    Some(Question { name, qtype, qclass, end: pos + 4 })
}

/// Build a response: header + echoed question + optional single answer.
fn build(query: &[u8], rcode: u8, question: &[u8], answer: Option<(u16, Vec<u8>)>) -> Vec<u8> {
    let rd = query[2] & 0x01;
    let opcode = query[2] & 0x78;
    let qd: u16 = if question.is_empty() { 0 } else { 1 };
    let an: u16 = answer.is_some() as u16;

    let mut p = Vec::with_capacity(64);
    p.extend_from_slice(&query[0..2]); // id
    p.push(0x80 | opcode | 0x04 | rd); // QR, opcode, AA, RD
    p.push(rcode & 0x0F); // RA = 0, no recursion
    p.extend_from_slice(&qd.to_be_bytes());
    p.extend_from_slice(&an.to_be_bytes());
    p.extend_from_slice(&[0, 0, 0, 0]); // no authority / additional
    p.extend_from_slice(question);

    if let Some((rtype, rdata)) = answer {
        p.extend_from_slice(&[0xC0, 0x0C]); // pointer to the name in the question
        p.extend_from_slice(&rtype.to_be_bytes());
        p.extend_from_slice(&CLASS_IN.to_be_bytes());
        p.extend_from_slice(&TTL_SECS.to_be_bytes());
        p.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        p.extend_from_slice(&rdata);
    }
    p
}

fn type_name(t: u16) -> String {
    match t {
        TYPE_A => "A".into(),
        TYPE_AAAA => "AAAA".into(),
        TYPE_ANY => "ANY".into(),
        n => format!("type{n}"),
    }
}

/// Returns None for packets that should be ignored (too short, or not a query).
pub fn handle(query: &[u8], matcher: &Matcher) -> Option<Reply> {
    if query.len() < 12 || query[2] & 0x80 != 0 {
        return None;
    }
    let qdcount = u16::from_be_bytes([query[4], query[5]]);
    let parsed = if qdcount == 1 { parse_question(query) } else { None };
    let Some(q) = parsed else {
        return Some(Reply {
            packet: build(query, RCODE_FORMERR, &[], None),
            summary: "malformed query".into(),
        });
    };

    let question = &query[12..q.end];
    let label = format!("{} {}", q.name, type_name(q.qtype));
    let opcode = (query[2] >> 3) & 0x0F;

    if opcode != 0 {
        return Some(Reply {
            packet: build(query, RCODE_NOTIMP, question, None),
            summary: format!("{label} -> NOTIMP"),
        });
    }
    if q.qclass != CLASS_IN {
        return Some(Reply {
            packet: build(query, RCODE_REFUSED, question, None),
            summary: format!("{label} -> REFUSED"),
        });
    }

    let Some((id, rule)) = matcher.find(&q.name) else {
        return Some(Reply {
            packet: build(query, RCODE_NXDOMAIN, question, None),
            summary: format!("{label} -> NXDOMAIN"),
        });
    };

    let answer = match (q.qtype, rule.address) {
        (TYPE_A | TYPE_ANY, IpAddr::V4(ip)) => Some((TYPE_A, ip.octets().to_vec())),
        (TYPE_AAAA | TYPE_ANY, IpAddr::V6(ip)) => Some((TYPE_AAAA, ip.octets().to_vec())),
        _ => None,
    };
    let summary = match &answer {
        Some(_) => format!("{label} -> {} (rule '{id}')", rule.address),
        None => format!("{label} -> no data (rule '{id}' is {})", rule.address),
    };
    Some(Reply { packet: build(query, RCODE_OK, question, answer), summary })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    fn query(name: &str, qtype: u16) -> Vec<u8> {
        let mut q = vec![0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
        for l in name.split('.') {
            q.push(l.len() as u8);
            q.extend_from_slice(l.as_bytes());
        }
        q.push(0);
        q.extend_from_slice(&qtype.to_be_bytes());
        q.extend_from_slice(&1u16.to_be_bytes());
        q
    }

    fn matcher() -> Matcher {
        let mut c = Config::default();
        c.add_rule("dev", "*.test", "127.0.0.1").unwrap();
        c.matcher()
    }

    #[test]
    fn answers_a_query_for_a_matching_name() {
        let r = handle(&query("Api.Shop.test", TYPE_A), &matcher()).unwrap();
        assert_eq!(&r.packet[0..2], &[0x12, 0x34]); // same id
        assert_eq!(r.packet[3] & 0x0F, RCODE_OK);
        assert_eq!(&r.packet[6..8], &[0, 1]); // one answer
        assert_eq!(&r.packet[r.packet.len() - 4..], &[127, 0, 0, 1]);
    }

    #[test]
    fn nxdomain_and_nodata() {
        let m = matcher();
        let r = handle(&query("example.com", TYPE_A), &m).unwrap();
        assert_eq!(r.packet[3] & 0x0F, RCODE_NXDOMAIN);

        let r = handle(&query("a.test", TYPE_AAAA), &m).unwrap(); // IPv4-only rule
        assert_eq!(r.packet[3] & 0x0F, RCODE_OK);
        assert_eq!(&r.packet[6..8], &[0, 0]); // no answers
    }

    #[test]
    fn garbage_does_not_panic() {
        let m = matcher();
        assert!(handle(&[1, 2, 3], &m).is_none());
        let mut bad = query("a.test", TYPE_A);
        bad.truncate(16);
        assert_eq!(handle(&bad, &m).unwrap().packet[3] & 0x0F, RCODE_FORMERR);
    }
}