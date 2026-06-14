use rust_decimal::Decimal;
use rust_decimal::MathematicalOps;
use rust_decimal_macros::dec;
use std::str::FromStr;

use super::action::CalcAction;
use super::token::*;

/// Result of a calculator action.
#[derive(Debug, Clone)]
pub struct CalcResult {
    pub display: String,
    pub history: String,
    pub memory_indicator: String,
    pub is_error: bool,
    pub events: Vec<VocalEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Input,
    OpPending,
    Evaluated,
    Error,
}

#[derive(Debug, Clone)]
enum MuCtx {
    None,
    Waiting { cost: Decimal },
    Done { cost: Decimal, sell: Decimal },
}

/// Old-style immediate-execution calculator engine.
pub struct Calculator {
    input: String,
    acc: Decimal,
    pending: Option<BinaryOp>,
    last_operand: Decimal,
    last_op: Option<BinaryOp>,
    pub memory: Decimal,
    state: State,
    mu: MuCtx,
    history: String,
}

impl Default for Calculator {
    fn default() -> Self {
        Self {
            input: String::new(),
            acc: Decimal::ZERO,
            pending: None,
            last_operand: Decimal::ZERO,
            last_op: None,
            memory: Decimal::ZERO,
            state: State::Idle,
            mu: MuCtx::None,
            history: String::new(),
        }
    }
}

impl Calculator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Reset internal state to match an incoming [`StateSnapshot`].
    ///
    /// Called when an authoritative `StateUpdate` arrives from a remote
    /// executor after speculative local execution.  Without this, the
    /// calculator's internal fields (`acc`, `pending`, `input`, `state`,
    /// …) remain in the speculative state and drift from the remote.
    ///
    /// The snapshot carries `display`, `history`, `memory_indicator`, and
    /// `is_error`.  From these we reconstruct the best approximation:
    ///
    /// - `acc` is parsed from the display string (falls back to 0).
    /// - `state` is set to `Error` when `is_error`, or `Evaluated` otherwise
    ///   (the most common post-result state).
    /// - `input`, `pending`, `last_operand`, `last_op`, and `mu` are cleared
    ///   because the snapshot does not carry them.
    /// - `memory` is left unchanged when the indicator is `"M"` (we cannot
    ///   recover the exact value), and zeroed when the indicator is empty.
    pub fn reset_from_snapshot(
        &mut self,
        display: &str,
        history: &str,
        memory_indicator: &str,
        is_error: bool,
    ) {
        if is_error {
            self.state = State::Error;
            self.acc = Decimal::ZERO;
        } else {
            self.state = State::Evaluated;
            self.acc = Decimal::from_str(display).unwrap_or(Decimal::ZERO);
        }
        self.input = String::new();
        self.pending = None;
        self.last_operand = Decimal::ZERO;
        self.last_op = None;
        self.mu = MuCtx::None;
        self.history = history.to_string();
        if memory_indicator.is_empty() {
            self.memory = Decimal::ZERO;
        }
        // When indicator is "M" we keep the existing `memory` value because
        // the snapshot does not carry the exact amount.
    }

    fn display_value(&self) -> Decimal {
        if self.input.is_empty() {
            self.acc
        } else {
            Decimal::from_str(&self.input).unwrap_or(self.acc)
        }
    }

    fn result(&self, events: Vec<VocalEvent>) -> CalcResult {
        let display = if self.state == State::Error {
            "错误".to_string()
        } else if self.input.is_empty() {
            if self.state == State::Idle {
                "0".to_string()
            } else {
                super::format::format_display(&self.acc)
            }
        } else {
            self.input.clone()
        };
        CalcResult {
            display,
            history: self.history.clone(),
            memory_indicator: if self.memory != Decimal::ZERO {
                "M"
            } else {
                ""
            }
            .to_string(),
            is_error: self.state == State::Error,
            events,
        }
    }

    fn enter_error(&mut self, err: CalcError) -> CalcResult {
        self.state = State::Error;
        self.history = err.to_string();
        self.result(vec![VocalEvent::Error(err)])
    }

    fn do_binary(&self, lhs: Decimal, op: BinaryOp, rhs: Decimal) -> Result<Decimal, CalcError> {
        match op {
            BinaryOp::Add => Ok(lhs + rhs),
            BinaryOp::Subtract => Ok(lhs - rhs),
            BinaryOp::Multiply => Ok(lhs * rhs),
            BinaryOp::Divide => {
                if rhs == Decimal::ZERO {
                    Err(CalcError::DivideByZero)
                } else {
                    Ok(lhs / rhs)
                }
            }
        }
    }

    fn eval_pending(&mut self) -> Result<Decimal, CalcError> {
        let rhs = self.display_value();
        if let Some(op) = self.pending {
            self.do_binary(self.acc, op, rhs)
        } else {
            Ok(rhs)
        }
    }

    // ---- public actions ----

    pub fn digit(&mut self, d: u8) -> CalcResult {
        if self.state == State::Error {
            return self.result(vec![]);
        }
        if self.state == State::Evaluated
            || self.state == State::Idle
            || self.state == State::OpPending
        {
            self.input.clear();
            self.state = State::Input;
        }
        self.input.push(char::from(b'0' + d));
        self.result(vec![VocalEvent::Digit(d)])
    }

    pub fn decimal_point(&mut self) -> CalcResult {
        if self.state == State::Error {
            return self.result(vec![]);
        }
        if self.state == State::Evaluated
            || self.state == State::Idle
            || self.state == State::OpPending
        {
            self.input = "0.".to_string();
            self.state = State::Input;
        } else if !self.input.contains('.') {
            if self.input.is_empty() {
                self.input = "0.".to_string();
            } else {
                self.input.push('.');
            }
        }
        self.result(vec![VocalEvent::DecimalPoint])
    }

    pub fn operator(&mut self, op: BinaryOp) -> CalcResult {
        if self.state == State::Error {
            return self.result(vec![]);
        }
        // evaluate pending operation
        if self.state == State::Input || self.state == State::Evaluated {
            match self.eval_pending() {
                Ok(val) => {
                    self.acc = val;
                    self.input.clear();
                    self.history =
                        format!("{} {}", super::format::format_display(&val), op.symbol());
                }
                Err(e) => return self.enter_error(e),
            }
        } else if self.state == State::OpPending {
            // just change operator
            self.history = format!(
                "{} {}",
                super::format::format_display(&self.acc),
                op.symbol()
            );
        }
        self.pending = Some(op);
        self.state = State::OpPending;
        self.mu = MuCtx::None;
        self.result(vec![VocalEvent::Operator(op)])
    }

    pub fn equals(&mut self) -> CalcResult {
        if self.state == State::Error {
            return self.result(vec![]);
        }

        // MU profit shortcut
        if let MuCtx::Done { cost, sell } = self.mu {
            let profit = sell - cost;
            self.acc = profit;
            self.input.clear();
            self.state = State::Evaluated;
            self.history = format!("{} = ", super::format::format_display(&sell));
            self.last_operand = cost;
            self.last_op = None;
            self.mu = MuCtx::None;
            return self.result(vec![VocalEvent::Equals, VocalEvent::Result(profit)]);
        }

        let (rhs, op) = if self.state == State::Evaluated {
            // repeat last operation
            if let Some(op) = self.last_op {
                (self.last_operand, op)
            } else {
                return self.result(vec![VocalEvent::Equals]);
            }
        } else if let Some(op) = self.pending {
            let rhs = if self.input.is_empty() {
                self.acc
            } else {
                self.display_value()
            };
            (rhs, op)
        } else {
            self.state = State::Evaluated;
            self.last_op = None;
            return self.result(vec![VocalEvent::Equals]);
        };

        match self.do_binary(self.acc, op, rhs) {
            Ok(val) => {
                self.history = format!(
                    "{} {} {} = ",
                    super::format::format_display(&self.acc),
                    op.symbol(),
                    super::format::format_display(&rhs)
                );
                self.acc = val;
                self.input.clear();
                self.last_operand = rhs;
                self.last_op = Some(op);
                self.pending = None;
                self.state = State::Evaluated;
                self.mu = MuCtx::None;
                self.result(vec![VocalEvent::Equals, VocalEvent::Result(val)])
            }
            Err(e) => self.enter_error(e),
        }
    }

    pub fn percent(&mut self) -> CalcResult {
        if self.state == State::Error {
            return self.result(vec![]);
        }

        // MU percent
        if let MuCtx::Waiting { cost } = self.mu {
            let rate = self.display_value();
            let denom = Decimal::ONE - rate / dec!(100);
            if denom == Decimal::ZERO {
                return self.enter_error(CalcError::DivideByZero);
            }
            let sell = cost / denom;
            self.acc = sell;
            self.input.clear();
            self.state = State::Evaluated;
            self.history = format!(
                "MU: {} / (1 - {}%)",
                super::format::format_display(&cost),
                super::format::format_display(&rate)
            );
            self.mu = MuCtx::Done { cost, sell };
            self.pending = None;
            return self.result(vec![VocalEvent::Percent, VocalEvent::Result(sell)]);
        }

        let b = self.display_value();
        let result = match self.pending {
            Some(BinaryOp::Add) => self.acc + self.acc * b / dec!(100),
            Some(BinaryOp::Subtract) => self.acc - self.acc * b / dec!(100),
            Some(BinaryOp::Multiply) => self.acc * b / dec!(100),
            Some(BinaryOp::Divide) => {
                let pct = b / dec!(100);
                if pct == Decimal::ZERO {
                    return self.enter_error(CalcError::DivideByZero);
                }
                self.acc / pct
            }
            None => b / dec!(100),
        };

        self.acc = result;
        self.input.clear();
        self.pending = None;
        self.state = State::Evaluated;
        self.result(vec![VocalEvent::Percent, VocalEvent::Result(result)])
    }

    pub fn mu(&mut self) -> CalcResult {
        if self.state == State::Error {
            return self.result(vec![]);
        }
        let cost = self.display_value();
        self.mu = MuCtx::Waiting { cost };
        self.input.clear();
        self.state = State::OpPending;
        self.result(vec![VocalEvent::MU])
    }

    pub fn square_root(&mut self) -> CalcResult {
        if self.state == State::Error {
            return self.result(vec![]);
        }
        let val = self.display_value();
        if val.is_sign_negative() {
            return self.enter_error(CalcError::NegativeSquareRoot);
        }
        match val.sqrt() {
            Some(root) => {
                self.acc = root;
                self.input.clear();
                self.state = State::Evaluated;
                self.result(vec![VocalEvent::SquareRoot, VocalEvent::Result(root)])
            }
            None => self.enter_error(CalcError::Overflow),
        }
    }

    pub fn backspace(&mut self) -> CalcResult {
        if self.state == State::Error {
            return self.result(vec![]);
        }
        if self.state == State::Input && !self.input.is_empty() {
            self.input.pop();
            if self.input.is_empty() || self.input == "-" || self.input == "-0" {
                self.input.clear();
                self.state = State::Idle;
            }
        }
        self.result(vec![VocalEvent::Backspace])
    }

    pub fn clear(&mut self) -> CalcResult {
        if self.state == State::Error {
            return self.result(vec![]);
        }
        self.input.clear();
        self.state = State::Idle;
        self.result(vec![VocalEvent::Clear])
    }

    pub fn all_clear(&mut self) -> CalcResult {
        let events = vec![VocalEvent::AllClear];
        self.reset();
        self.result(events)
    }

    pub fn plus_minus(&mut self) -> CalcResult {
        if self.state == State::Error {
            return self.result(vec![]);
        }
        if self.state == State::Input && !self.input.is_empty() {
            if self.input.starts_with('-') {
                self.input.remove(0);
                self.result(vec![VocalEvent::SignPositive])
            } else {
                self.input.insert(0, '-');
                self.result(vec![VocalEvent::SignNegative])
            }
        } else {
            self.acc = -self.acc;
            if self.acc.is_sign_negative() {
                self.result(vec![VocalEvent::SignNegative])
            } else {
                self.result(vec![VocalEvent::SignPositive])
            }
        }
    }

    pub fn memory_recall(&mut self) -> CalcResult {
        if self.state == State::Error {
            return self.result(vec![]);
        }
        self.acc = self.memory;
        self.input.clear();
        self.state = State::Evaluated;
        self.result(vec![
            VocalEvent::MemoryRecall,
            VocalEvent::Result(self.memory),
        ])
    }

    pub fn memory_add(&mut self) -> CalcResult {
        if self.state == State::Error {
            return self.result(vec![]);
        }
        self.memory += self.display_value();
        self.state = State::Evaluated;
        self.result(vec![VocalEvent::MemoryAdd])
    }

    pub fn memory_subtract(&mut self) -> CalcResult {
        if self.state == State::Error {
            return self.result(vec![]);
        }
        self.memory -= self.display_value();
        self.state = State::Evaluated;
        self.result(vec![VocalEvent::MemorySubtract])
    }

    pub fn memory_clear(&mut self) -> CalcResult {
        if self.state == State::Error {
            return self.result(vec![]);
        }
        self.memory = Decimal::ZERO;
        self.state = State::Evaluated;
        self.result(vec![VocalEvent::MemoryClear])
    }

    /// Dispatch a serializable action to the corresponding calculator method.
    pub fn dispatch(&mut self, action: CalcAction) -> CalcResult {
        match action {
            CalcAction::Digit(d) => self.digit(d),
            CalcAction::DecimalPoint => self.decimal_point(),
            CalcAction::Operator(op) => self.operator(op),
            CalcAction::Equals => self.equals(),
            CalcAction::Percent => self.percent(),
            CalcAction::Mu => self.mu(),
            CalcAction::SquareRoot => self.square_root(),
            CalcAction::Backspace => self.backspace(),
            CalcAction::Clear => self.clear(),
            CalcAction::AllClear => self.all_clear(),
            CalcAction::PlusMinus => self.plus_minus(),
            CalcAction::MemoryRecall => self.memory_recall(),
            CalcAction::MemoryAdd => self.memory_add(),
            CalcAction::MemorySubtract => self.memory_subtract(),
            CalcAction::MemoryClear => self.memory_clear(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn display(calc: &mut Calculator, actions: &[&str]) -> String {
        let mut r = CalcResult {
            display: "0".into(),
            history: String::new(),
            memory_indicator: String::new(),
            is_error: false,
            events: vec![],
        };
        for a in actions {
            r = match *a {
                "0" => calc.digit(0),
                "1" => calc.digit(1),
                "2" => calc.digit(2),
                "3" => calc.digit(3),
                "4" => calc.digit(4),
                "5" => calc.digit(5),
                "6" => calc.digit(6),
                "7" => calc.digit(7),
                "8" => calc.digit(8),
                "9" => calc.digit(9),
                "." => calc.decimal_point(),
                "+" => calc.operator(BinaryOp::Add),
                "-" => calc.operator(BinaryOp::Subtract),
                "*" => calc.operator(BinaryOp::Multiply),
                "/" => calc.operator(BinaryOp::Divide),
                "=" => calc.equals(),
                "%" => calc.percent(),
                "mu" => calc.mu(),
                "sqrt" => calc.square_root(),
                "bs" => calc.backspace(),
                "c" => calc.clear(),
                "ac" => calc.all_clear(),
                "+-" => calc.plus_minus(),
                "mr" => calc.memory_recall(),
                "m+" => calc.memory_add(),
                "m-" => calc.memory_subtract(),
                "mc" => calc.memory_clear(),
                _ => panic!("unknown action: {}", a),
            };
        }
        r.display
    }

    #[test]
    fn basic_addition() {
        let mut c = Calculator::new();
        assert_eq!(display(&mut c, &["2", "+", "3", "="]), "5");
    }

    #[test]
    fn chain_operations() {
        let mut c = Calculator::new();
        assert_eq!(display(&mut c, &["2", "+", "3", "+", "4", "="]), "9");
    }

    #[test]
    fn repeat_equals() {
        let mut c = Calculator::new();
        assert_eq!(display(&mut c, &["5", "+", "3", "=", "=", "="]), "14");
    }

    #[test]
    fn percent_add() {
        let mut c = Calculator::new();
        // 10 + 20% = 10 + 10*20/100 = 12
        assert_eq!(display(&mut c, &["1", "0", "+", "2", "0", "%"]), "12");
    }

    #[test]
    fn percent_subtract() {
        let mut c = Calculator::new();
        // 100 - 20% = 100 - 100*20/100 = 80
        assert_eq!(display(&mut c, &["1", "0", "0", "-", "2", "0", "%"]), "80");
    }

    #[test]
    fn percent_multiply() {
        let mut c = Calculator::new();
        // 200 * 15% = 200*15/100 = 30
        assert_eq!(display(&mut c, &["2", "0", "0", "*", "1", "5", "%"]), "30");
    }

    #[test]
    fn percent_divide() {
        let mut c = Calculator::new();
        // 50 / 25% = 50 / (25/100) = 200
        assert_eq!(display(&mut c, &["5", "0", "/", "2", "5", "%"]), "200");
    }

    #[test]
    fn percent_standalone() {
        let mut c = Calculator::new();
        // 50% = 0.5
        assert_eq!(display(&mut c, &["5", "0", "%"]), "0.5");
    }

    #[test]
    fn mu_basic() {
        let mut c = Calculator::new();
        // 120 MU 25 % => 160
        assert_eq!(
            display(&mut c, &["1", "2", "0", "mu", "2", "5", "%"]),
            "160"
        );
        // = => 40 (profit)
        assert_eq!(display(&mut c, &["="]), "40");
    }

    #[test]
    fn divide_by_zero() {
        let mut c = Calculator::new();
        let r = display(&mut c, &["5", "/", "0", "="]);
        assert_eq!(r, "错误");
    }

    #[test]
    fn sqrt_negative() {
        let mut c = Calculator::new();
        let r = display(&mut c, &["9", "+-", "sqrt"]);
        assert_eq!(r, "错误");
    }

    #[test]
    fn sqrt_basic() {
        let mut c = Calculator::new();
        assert_eq!(display(&mut c, &["9", "sqrt"]), "3");
    }

    #[test]
    fn memory_operations() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "m+"]);
        display(&mut c, &["3", "m+"]);
        assert_eq!(display(&mut c, &["mr"]), "8");
        display(&mut c, &["2", "m-"]);
        assert_eq!(display(&mut c, &["mr"]), "6");
        display(&mut c, &["mc"]);
        assert_eq!(display(&mut c, &["mr"]), "0");
    }

    #[test]
    fn backspace() {
        let mut c = Calculator::new();
        display(&mut c, &["1", "2", "3"]);
        assert_eq!(display(&mut c, &["bs"]), "12");
    }

    #[test]
    fn clear_and_all_clear() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "+", "3"]);
        assert_eq!(display(&mut c, &["c"]), "0");
        display(&mut c, &["1", "+", "2", "="]);
        assert_eq!(display(&mut c, &["ac"]), "0");
    }

    #[test]
    fn decimal_input() {
        let mut c = Calculator::new();
        display(&mut c, &["3", ".", "1", "4"]);
        assert_eq!(display(&mut c, &["+"]), "3.14");
    }

    #[test]
    fn plus_minus() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "+-"]);
        assert_eq!(display(&mut c, &["+"]), "-5");
    }

    #[test]
    fn no_floating_point() {
        let mut c = Calculator::new();
        // 0.1 + 0.2 should be exactly 0.3
        assert_eq!(
            display(&mut c, &["0", ".", "1", "+", "0", ".", "2", "="]),
            "0.3"
        );
    }

    #[test]
    fn dispatch_matches_direct_calls() {
        let mut c1 = Calculator::new();
        let mut c2 = Calculator::new();

        let actions = [
            CalcAction::Digit(5),
            CalcAction::Operator(BinaryOp::Add),
            CalcAction::Digit(3),
            CalcAction::Equals,
        ];
        let direct = [
            c1.digit(5),
            c1.operator(BinaryOp::Add),
            c1.digit(3),
            c1.equals(),
        ];
        for (i, action) in actions.iter().enumerate() {
            let r = c2.dispatch(*action);
            assert_eq!(r.display, direct[i].display, "action {} mismatch", i);
        }
    }

    #[test]
    fn dispatch_all_variants_compile() {
        // Verify every CalcAction variant dispatches without panicking.
        let mut c = Calculator::new();
        let actions = [
            CalcAction::Digit(1),
            CalcAction::DecimalPoint,
            CalcAction::Digit(5),
            CalcAction::Operator(BinaryOp::Add),
            CalcAction::Digit(2),
            CalcAction::Equals,
            CalcAction::Percent,
            CalcAction::Mu,
            CalcAction::SquareRoot,
            CalcAction::Backspace,
            CalcAction::Clear,
            CalcAction::AllClear,
            CalcAction::PlusMinus,
            CalcAction::MemoryRecall,
            CalcAction::MemoryAdd,
            CalcAction::MemorySubtract,
            CalcAction::MemoryClear,
        ];
        for action in actions {
            c.dispatch(action);
        }
    }

    // ------ Edge-case tests ------

    #[test]
    fn error_state_digit_returns_empty_events() {
        let mut c = Calculator::new();
        // Trigger divide-by-zero error.
        display(&mut c, &["5", "/", "0", "="]);
        let r = c.digit(3);
        assert!(r.is_error, "should remain in error state");
        assert!(r.events.is_empty(), "digit in error state should produce no events");
        assert_eq!(r.display, "错误");
    }

    #[test]
    fn error_state_operator_returns_empty_events() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "/", "0", "="]);
        let r = c.operator(BinaryOp::Add);
        assert!(r.is_error);
        assert!(r.events.is_empty());
    }

    #[test]
    fn error_state_equals_returns_empty_events() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "/", "0", "="]);
        let r = c.equals();
        assert!(r.is_error);
        assert!(r.events.is_empty());
    }

    #[test]
    fn error_state_clear_returns_empty_events() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "/", "0", "="]);
        // clear itself returns Clear event (it does NOT early-return in Error)
        // Wait -- it DOES early-return. Let's check.
        let r = c.clear();
        assert!(r.is_error, "clear in error state should remain error");
        assert!(r.events.is_empty());
    }

    #[test]
    fn error_state_all_clear_recovers() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "/", "0", "="]);
        let r = c.all_clear();
        assert!(!r.is_error, "all_clear should exit error state");
        assert_eq!(r.display, "0");
        assert_eq!(r.events, vec![VocalEvent::AllClear]);
    }

    #[test]
    fn error_state_memory_recall_returns_empty_events() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "/", "0", "="]);
        let r = c.memory_recall();
        assert!(r.is_error);
        assert!(r.events.is_empty());
    }

    #[test]
    fn error_state_memory_add_returns_empty_events() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "/", "0", "="]);
        let r = c.memory_add();
        assert!(r.is_error);
        assert!(r.events.is_empty());
    }

    #[test]
    fn error_state_memory_subtract_returns_empty_events() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "/", "0", "="]);
        let r = c.memory_subtract();
        assert!(r.is_error);
        assert!(r.events.is_empty());
    }

    #[test]
    fn error_state_memory_clear_returns_empty_events() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "/", "0", "="]);
        let r = c.memory_clear();
        assert!(r.is_error);
        assert!(r.events.is_empty());
    }

    #[test]
    fn error_state_backspace_returns_empty_events() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "/", "0", "="]);
        let r = c.backspace();
        assert!(r.is_error);
        assert!(r.events.is_empty());
    }

    #[test]
    fn error_state_decimal_point_returns_empty_events() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "/", "0", "="]);
        let r = c.decimal_point();
        assert!(r.is_error);
        assert!(r.events.is_empty());
    }

    #[test]
    fn error_state_percent_returns_empty_events() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "/", "0", "="]);
        let r = c.percent();
        assert!(r.is_error);
        assert!(r.events.is_empty());
    }

    #[test]
    fn error_state_square_root_returns_empty_events() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "/", "0", "="]);
        let r = c.square_root();
        assert!(r.is_error);
        assert!(r.events.is_empty());
    }

    #[test]
    fn error_state_mu_returns_empty_events() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "/", "0", "="]);
        let r = c.mu();
        assert!(r.is_error);
        assert!(r.events.is_empty());
    }

    #[test]
    fn error_state_plus_minus_returns_empty_events() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "/", "0", "="]);
        let r = c.plus_minus();
        assert!(r.is_error);
        assert!(r.events.is_empty());
    }

    #[test]
    fn double_decimal_point_is_noop() {
        let mut c = Calculator::new();
        // Enter "3." then press "." again.
        display(&mut c, &["3", "."]);
        let r = c.decimal_point();
        assert_eq!(r.display, "3.", "second decimal point should not change display");
        // The event is still emitted (design choice), but the input must not have two dots.
        assert_eq!(r.events, vec![VocalEvent::DecimalPoint]);
    }

    #[test]
    fn double_decimal_point_from_idle() {
        let mut c = Calculator::new();
        // Starting from idle: first "." gives "0.", second "." is no-op on input.
        let r1 = c.decimal_point();
        assert_eq!(r1.display, "0.");
        let r2 = c.decimal_point();
        assert_eq!(r2.display, "0.");
    }

    #[test]
    fn operator_with_empty_input_uses_acc_as_implicit_operand() {
        let mut c = Calculator::new();
        // Enter 10, press + (acc=10, pending=Add, state=OpPending).
        display(&mut c, &["1", "0", "+"]);
        // Press + again without entering a digit -- this changes operator but
        // the implicit operand is still acc (10).
        // Then enter 3 and evaluate.
        assert_eq!(display(&mut c, &["3", "="]), "13");
    }

    #[test]
    fn operator_chaining_in_op_pending_replaces_operator() {
        let mut c = Calculator::new();
        // 5 + (OpPending) then change to * before entering rhs.
        display(&mut c, &["5", "+"]);
        // Change operator to multiply.
        let r = c.operator(BinaryOp::Multiply);
        assert_eq!(r.display, "5", "display should remain acc during operator change");
        // Now enter 4 and evaluate: 5 * 4 = 20.
        assert_eq!(display(&mut c, &["4", "="]), "20");
    }

    #[test]
    fn operator_from_idle_uses_zero_acc() {
        let mut c = Calculator::new();
        // Press "+" from idle with no prior input. acc=0, pending=Add.
        display(&mut c, &["+"]);
        // Enter 7 and press =: 0 + 7 = 7.
        assert_eq!(display(&mut c, &["7", "="]), "7");
    }

    #[test]
    fn equals_with_no_pending_operation_is_noop() {
        let mut c = Calculator::new();
        // From idle, just press "=". No pending op, no last_op.
        let r = c.equals();
        assert_eq!(r.display, "0");
        assert_eq!(r.events, vec![VocalEvent::Equals]);
        assert!(!r.is_error);
    }

    #[test]
    fn equals_after_evaluated_with_no_last_op_is_noop() {
        let mut c = Calculator::new();
        // Enter a number and press equals without any operator.
        display(&mut c, &["4", "2", "="]);
        // Now in Evaluated state, last_op is None. Press "=" again.
        let r = c.equals();
        assert_eq!(r.display, "42");
        assert_eq!(r.events, vec![VocalEvent::Equals]);
    }

    #[test]
    fn memory_add_in_op_pending_state() {
        let mut c = Calculator::new();
        // 5 + (OpPending), then M+. display_value is acc (5) since input is empty.
        display(&mut c, &["5", "+"]);
        let r = c.memory_add();
        assert_eq!(r.memory_indicator, "M");
        assert_eq!(c.memory.to_string(), "5");
    }

    #[test]
    fn memory_subtract_when_memory_is_zero() {
        let mut c = Calculator::new();
        display(&mut c, &["3"]);
        let r = c.memory_subtract();
        // memory is now -3, nonzero, so indicator is "M".
        assert_eq!(r.memory_indicator, "M");
        assert_eq!(c.memory.to_string(), "-3");
    }

    #[test]
    fn memory_recall_sets_acc_to_memory() {
        let mut c = Calculator::new();
        display(&mut c, &["1", "0", "0", "m+"]);
        // Clear state and recall.
        display(&mut c, &["c"]);
        let r = c.memory_recall();
        assert_eq!(r.display, "100");
        assert_eq!(
            r.events,
            vec![VocalEvent::MemoryRecall, VocalEvent::Result(dec!(100))]
        );
    }

    #[test]
    fn memory_recall_in_error_state() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "m+"]);
        display(&mut c, &["5", "/", "0", "="]);
        let r = c.memory_recall();
        assert!(r.is_error);
        assert!(r.events.is_empty());
    }

    #[test]
    fn memory_clear_resets_indicator() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "m+"]);
        assert_eq!(c.memory.to_string(), "5");
        let r = c.memory_clear();
        assert_eq!(r.memory_indicator, "");
        assert_eq!(c.memory.to_string(), "0");
    }

    #[test]
    fn backspace_single_digit_goes_idle() {
        let mut c = Calculator::new();
        display(&mut c, &["5"]);
        let r = c.backspace();
        assert_eq!(r.display, "0", "backspace on single digit should return to idle showing 0");
        // After backspace, state is Idle: next digit starts fresh input.
        assert_eq!(display(&mut c, &["3"]), "3");
    }

    #[test]
    fn backspace_leaves_negative_sign_goes_idle() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "+-"]);
        // input is "-5"
        let r = c.backspace();
        // After popping last char, input is "-", which matches the "-" check -> clear, go idle.
        assert_eq!(r.display, "0");
    }

    #[test]
    fn backspace_leaves_negative_zero_goes_idle() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "+-"]);
        // input is "-5", pop -> "-", not "-0" so idle. Let's test the "-0" path:
        // We need "-0" in input. Press "+-", input is "-5" not "-0".
        // To get "-0", we need: 0 +- => "-0" -- nope, plus_minus on "0" makes it "-0"?
        // Actually: digit(0) -> input="0", plus_minus -> input="-0".
        display(&mut c, &["c"]);
        display(&mut c, &["0", "+-"]);
        // input should be "-0"
        let r = c.backspace();
        assert_eq!(r.display, "0", "backspace on '-0' should return to idle");
    }

    #[test]
    fn backspace_after_operator_is_noop() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "+"]);
        // State is OpPending, not Input. Backspace should not change anything.
        let r = c.backspace();
        assert_eq!(r.display, "5", "backspace after operator should be no-op on display");
        assert_eq!(r.events, vec![VocalEvent::Backspace]);
    }

    #[test]
    fn backspace_after_evaluated_is_noop() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "+", "3", "="]);
        // State is Evaluated. Backspace should not change anything.
        let r = c.backspace();
        assert_eq!(r.display, "8", "backspace after equals should be no-op on display");
    }

    #[test]
    fn backspace_in_idle_is_noop() {
        let mut c = Calculator::new();
        let r = c.backspace();
        assert_eq!(r.display, "0", "backspace in idle state should be no-op");
    }

    #[test]
    fn clear_after_divide_by_zero_stays_error() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "/", "0", "="]);
        let r = c.clear();
        assert!(r.is_error, "clear in error state should keep error flag");
        assert!(r.events.is_empty());
        assert_eq!(r.display, "错误");
    }

    #[test]
    fn negative_sqrt_error_state_empty_events() {
        let mut c = Calculator::new();
        display(&mut c, &["9", "+-"]);
        let r = c.square_root();
        assert!(r.is_error);
        // enter_error wraps the error in a single Error event.
        assert_eq!(r.events, vec![VocalEvent::Error(CalcError::NegativeSquareRoot)]);
        // Subsequent actions in error state return empty.
        let r2 = c.digit(1);
        assert!(r2.is_error);
        assert!(r2.events.is_empty());
    }

    // ------ reset_from_snapshot tests ------

    #[test]
    fn reset_from_snapshot_sets_acc_from_display() {
        let mut c = Calculator::new();
        // Build up some state: 9 + 3 = → acc=12
        let r = display(&mut c, &["9", "+", "3", "="]);
        assert_eq!(r, "12");

        // Reset from a snapshot with display "99".
        c.reset_from_snapshot("99", "90 + 9 = ", "", false);

        // After reset, dispatching "+ 1 =" should give 100, not 13.
        let r = c.operator(BinaryOp::Add);
        assert_eq!(r.display, "99", "acc should be 99 after reset");
        let _r = c.digit(1);
        let r = c.equals();
        assert_eq!(r.display, "100");
    }

    #[test]
    fn reset_from_snapshot_clears_pending_and_input() {
        let mut c = Calculator::new();
        // Leave in OpPending state: 5 +
        display(&mut c, &["5", "+"]);

        c.reset_from_snapshot("42", "40 + 2 = ", "", false);

        // Pending should be cleared -- pressing "=" should be a no-op (no pending op).
        let r = c.equals();
        assert_eq!(r.display, "42", "no pending op after reset, equals should keep display");
    }

    #[test]
    fn reset_from_snapshot_sets_error_state() {
        let mut c = Calculator::new();
        display(&mut c, &["1", "+", "2", "="]);

        c.reset_from_snapshot("错误", "不能除以零", "", true);

        assert!(c.result(vec![]).is_error, "should be in error state after reset");
        // Actions in error state should return empty events.
        let r = c.digit(5);
        assert!(r.is_error);
        assert!(r.events.is_empty());
    }

    #[test]
    fn reset_from_snapshot_clears_error_state() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "/", "0", "="]);
        assert!(c.result(vec![]).is_error);

        c.reset_from_snapshot("0", "", "", false);

        let r = c.digit(1);
        assert!(!r.is_error, "error should be cleared after reset");
        assert_eq!(r.display, "1");
    }

    #[test]
    fn reset_from_snapshot_zeroes_memory_when_indicator_empty() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "m+"]);
        assert_ne!(c.memory, Decimal::ZERO);

        c.reset_from_snapshot("0", "", "", false);

        assert_eq!(c.memory, Decimal::ZERO, "memory should be zeroed when indicator is empty");
    }

    #[test]
    fn reset_from_snapshot_keeps_memory_when_indicator_m() {
        let mut c = Calculator::new();
        display(&mut c, &["5", "m+"]);
        let saved_memory = c.memory;

        c.reset_from_snapshot("0", "", "M", false);

        assert_eq!(c.memory, saved_memory, "memory should be preserved when indicator is 'M'");
    }

    #[test]
    fn reset_from_snapshot_sets_history() {
        let mut c = Calculator::new();

        c.reset_from_snapshot("42", "40 + 2 = ", "", false);

        let r = c.result(vec![]);
        assert_eq!(r.history, "40 + 2 = ");
    }

    #[test]
    fn reset_from_snapshot_handles_zero_display() {
        let mut c = Calculator::new();
        display(&mut c, &["9", "+", "3", "="]);

        c.reset_from_snapshot("0", "", "", false);

        let r = c.equals();
        assert_eq!(r.display, "0", "display should be 0 after reset to zero");
    }
}
