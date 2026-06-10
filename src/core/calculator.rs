use rust_decimal::Decimal;
use rust_decimal::MathematicalOps;
use rust_decimal_macros::dec;
use std::str::FromStr;

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

    fn display_value(&self) -> Decimal {
        if self.input.is_empty() {
            self.acc
        } else {
            Decimal::from_str(&self.input).unwrap_or(self.acc)
        }
    }

    fn result(&self, events: Vec<VocalEvent>) -> CalcResult {
        let display = if self.state == State::Error {
            "Error".to_string()
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
        assert_eq!(r, "Error");
    }

    #[test]
    fn sqrt_negative() {
        let mut c = Calculator::new();
        let r = display(&mut c, &["9", "+-", "sqrt"]);
        assert_eq!(r, "Error");
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
}
