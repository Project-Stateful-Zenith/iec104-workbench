use iec104::apci::{self, Apci, SeqNo};

use crate::telegram::Dir;

/// The link's sequence numbers, followed from the traced frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sequence {
    /// V(S): the next I-frame sent gets this N(S).
    pub send: SeqNo,
    /// V(R): the N(S) expected on the next I-frame received.
    pub receive: SeqNo,
    acknowledged: SeqNo,
}

impl Default for Sequence {
    fn default() -> Self {
        Self {
            send: SeqNo::new(0),
            receive: SeqNo::new(0),
            acknowledged: SeqNo::new(0),
        }
    }
}

impl Sequence {
    pub fn trace(&mut self, dir: Dir, frame: &[u8]) {
        let Ok((apdu, _)) = apci::parse(frame) else {
            return;
        };
        match (dir, apdu.apci) {
            (Dir::Tx, Apci::I { ns, .. }) => self.send = ns.next(),
            (Dir::Rx, Apci::I { ns, nr }) => {
                self.receive = ns.next();
                self.acknowledged = nr;
            }
            (Dir::Rx, Apci::S { nr }) => self.acknowledged = nr,
            _ => {}
        }
    }

    /// I-frames sent that the station has not acknowledged yet.
    pub const fn unacknowledged(self) -> u16 {
        self.send.get().wrapping_sub(self.acknowledged.get()) & 0x7FFF
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An I-frame carrying an interrogation command.
    fn i_frame(ns: u16, nr: u16) -> Vec<u8> {
        let mut frame = vec![0x68, 0x0E];
        frame.extend((ns << 1).to_le_bytes());
        frame.extend((nr << 1).to_le_bytes());
        frame.extend([0x64, 0x01, 0x06, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x14]);
        frame
    }

    fn s_frame(nr: u16) -> Vec<u8> {
        let [lo, hi] = (nr << 1).to_le_bytes();
        vec![0x68, 0x04, 0x01, 0x00, lo, hi]
    }

    #[test]
    fn follows_send_receive_and_acknowledgement() {
        let mut seq = Sequence::default();
        seq.trace(Dir::Tx, &i_frame(0, 0));
        seq.trace(Dir::Tx, &i_frame(1, 0));
        assert_eq!((seq.send.get(), seq.unacknowledged()), (2, 2));
        seq.trace(Dir::Rx, &i_frame(0, 1));
        assert_eq!((seq.receive.get(), seq.unacknowledged()), (1, 1));
        seq.trace(Dir::Rx, &s_frame(2));
        assert_eq!(seq.unacknowledged(), 0);
        // U-frames carry no numbers.
        seq.trace(Dir::Rx, &[0x68, 0x04, 0x0B, 0x00, 0x00, 0x00]);
        assert_eq!(seq.receive.get(), 1);
    }

    #[test]
    fn wraps_at_32767() {
        let mut seq = Sequence::default();
        seq.trace(Dir::Tx, &i_frame(32766, 0));
        seq.trace(Dir::Rx, &s_frame(32767));
        assert_eq!(seq.unacknowledged(), 0);
        seq.trace(Dir::Tx, &i_frame(32767, 0));
        seq.trace(Dir::Tx, &i_frame(0, 0));
        assert_eq!((seq.send.get(), seq.unacknowledged()), (1, 2));
        seq.trace(Dir::Rx, &i_frame(32767, 0));
        assert_eq!(seq.receive.get(), 0);
    }
}
