use rust_decimal::Decimal;

/// Binary arithmetic operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
}

impl BinaryOp {
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Subtract => "-",
            Self::Multiply => "\u{00d7}",
            Self::Divide => "\u{00f7}",
        }
    }
}

/// Calculation error kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CalcError {
    DivideByZero,
    NegativeSquareRoot,
    Overflow,
}

impl std::fmt::Display for CalcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DivideByZero => write!(f, "不能除以零"),
            Self::NegativeSquareRoot => write!(f, "输入无效"),
            Self::Overflow => write!(f, "溢出"),
        }
    }
}

/// Semantic vocal events produced by calculator actions.
///
/// Each event represents an action the audio system should announce.
/// The audio mode determines how each event is rendered into sound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VocalEvent {
    /// A digit was pressed (0-9).
    Digit(u8),
    /// Decimal point was pressed.
    DecimalPoint,
    /// An operator was pressed or announced.
    Operator(BinaryOp),
    /// Equals was pressed.
    Equals,
    /// Percent was pressed.
    Percent,
    /// MU (markup) was pressed.
    MU,
    /// Square root was pressed.
    SquareRoot,
    /// Backspace was pressed.
    Backspace,
    /// Clear input was pressed.
    Clear,
    /// All-clear was pressed.
    AllClear,
    /// Memory recall.
    MemoryRecall,
    /// Memory add.
    MemoryAdd,
    /// Memory subtract.
    MemorySubtract,
    /// Memory clear.
    MemoryClear,
    /// Value became negative after sign toggle.
    SignNegative,
    /// Value became positive after sign toggle.
    SignPositive,
    /// An error occurred.
    Error(CalcError),
    /// A result value should be spoken (contains the value for speech decomposition).
    Result(Decimal),
}
