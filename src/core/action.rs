use serde::{Deserialize, Serialize};

use super::token::BinaryOp;

/// Serializable action that maps 1:1 to Calculator public methods.
///
/// This enum serves as the canonical representation of user intent,
/// suitable for serialization (network transmission, logging, replay).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CalcAction {
    Digit(u8),
    DecimalPoint,
    Operator(BinaryOp),
    Equals,
    Percent,
    Mu,
    SquareRoot,
    Backspace,
    Clear,
    AllClear,
    PlusMinus,
    MemoryRecall,
    MemoryAdd,
    MemorySubtract,
    MemoryClear,
}
