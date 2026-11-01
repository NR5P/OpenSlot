//! HamPlay, VarAC's games over a QSO (tic-tac-toe, Connect Four, the game
//! center). Wire: `<P:n>` invite, `<PR>` accept, `<PJ>` reject, `<PM:x>` move, `<PA>` left the
//! game, `<PE>` rematch. The inviter (or the one asking for the rematch) is player 1 (X / blue)
//! and moves first. There is no end-of-game message: both sides find the winner themselves.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Game {
    /// id 1: "Toc-Tac-Toe" (sic), "Classic version (3X3)"
    TicTacToe,
    /// id 2: "Connect Four", "Be the first to form a line of 4 discs"
    ConnectFour,
}

impl Game {
    pub fn from_id(id: u8) -> Option<Self> {
        match id {
            1 => Some(Game::TicTacToe),
            2 => Some(Game::ConnectFour),
            _ => None,
        }
    }
    pub fn id(self) -> u8 {
        match self {
            Game::TicTacToe => 1,
            Game::ConnectFour => 2,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Game::TicTacToe => "Tic-Tac-Toe",
            Game::ConnectFour => "Connect Four",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Game::TicTacToe => "Classic version (3X3)",
            Game::ConnectFour => "Be the first to form a line of 4 discs",
        }
    }
    fn size(self) -> (usize, usize) {
        match self {
            Game::TicTacToe => (3, 3),
            Game::ConnectFour => (6, 7),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Win(u8),
    Draw,
}

/// One game. Players are 1 and 2; cells hold 0 (empty), 1 or 2.
#[derive(Debug, Clone)]
pub struct Board {
    pub game: Game,
    pub cells: Vec<Vec<u8>>,
    pub to_move: u8,
    pub outcome: Option<Outcome>,
    /// Last cell played (row, column), for highlighting
    pub last: Option<(usize, usize)>,
}

impl Board {
    pub fn new(game: Game) -> Self {
        let (r, c) = game.size();
        Board { game, cells: vec![vec![0; c]; r], to_move: 1, outcome: None, last: None }
    }

    /// The move text for a click on (row, column): `RC` for tic-tac-toe, `C` for Connect Four.
    /// None when that move isn't possible (taken cell, full column, game over).
    pub fn move_for_click(&self, row: usize, col: usize) -> Option<String> {
        if self.outcome.is_some() {
            return None;
        }
        match self.game {
            Game::TicTacToe => (self.cells.get(row)?.get(col)? == &0).then(|| format!("{row}{col}")),
            Game::ConnectFour => (col < 7 && self.cells[0][col] == 0).then(|| col.to_string()),
        }
    }

    /// Play `mv` for the player to move. Returns false for a move that doesn't fit.
    pub fn play(&mut self, mv: &str) -> bool {
        if self.outcome.is_some() {
            return false;
        }
        let digits: Vec<usize> = mv.trim().chars().filter_map(|c| c.to_digit(10).map(|d| d as usize)).collect();
        let cell = match (self.game, digits.as_slice()) {
            (Game::TicTacToe, [r, c]) if *r < 3 && *c < 3 && self.cells[*r][*c] == 0 => (*r, *c),
            (Game::ConnectFour, [c]) if *c < 7 => match (0..6).rev().find(|&r| self.cells[r][*c] == 0) {
                Some(r) => (r, *c),
                None => return false,
            },
            _ => return false,
        };
        let p = self.to_move;
        self.cells[cell.0][cell.1] = p;
        self.last = Some(cell);
        self.outcome = self.check(cell, p);
        self.to_move = 3 - p;
        true
    }

    fn check(&self, (r, c): (usize, usize), p: u8) -> Option<Outcome> {
        let need = match self.game {
            Game::TicTacToe => 3,
            Game::ConnectFour => 4,
        };
        let (rows, cols) = self.game.size();
        let at = |r: isize, c: isize| r >= 0 && c >= 0 && (r as usize) < rows && (c as usize) < cols && self.cells[r as usize][c as usize] == p;
        for (dr, dc) in [(0isize, 1isize), (1, 0), (1, 1), (1, -1)] {
            let mut n = 1;
            for sign in [1isize, -1] {
                let (mut rr, mut cc) = (r as isize + dr * sign, c as isize + dc * sign);
                while at(rr, cc) {
                    n += 1;
                    rr += dr * sign;
                    cc += dc * sign;
                }
            }
            if n >= need {
                return Some(Outcome::Win(p));
            }
        }
        // Draw: tic-tac-toe all filled; Connect Four the top row full
        let full = match self.game {
            Game::TicTacToe => self.cells.iter().all(|row| row.iter().all(|&v| v != 0)),
            Game::ConnectFour => self.cells[0].iter().all(|&v| v != 0),
        };
        full.then_some(Outcome::Draw)
    }

    pub fn symbol(game: Game, player: u8) -> &'static str {
        match (game, player) {
            (Game::TicTacToe, 1) => "X",
            (Game::TicTacToe, 2) => "O",
            (Game::ConnectFour, 1) => "BLUE",
            (Game::ConnectFour, 2) => "ORANGE",
            _ => "",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tic_tac_toe() {
        let mut b = Board::new(Game::TicTacToe);
        for mv in ["00", "11", "01", "22"] {
            assert!(b.play(mv));
        }
        assert!(!b.play("00"), "taken");
        assert_eq!(b.move_for_click(1, 1), None);
        assert_eq!(b.move_for_click(0, 2).as_deref(), Some("02"));
        assert!(b.play("02"));
        assert_eq!(b.outcome, Some(Outcome::Win(1)));
        let mut d = Board::new(Game::TicTacToe);
        for mv in ["00", "01", "02", "11", "10", "12", "21", "20", "22"] {
            assert!(d.play(mv));
        }
        assert_eq!(d.outcome, Some(Outcome::Draw));
    }

    #[test]
    fn connect_four() {
        let mut b = Board::new(Game::ConnectFour);
        // discs drop to the bottom
        assert!(b.play("3"));
        assert_eq!(b.cells[5][3], 1);
        assert!(b.play("3"));
        assert_eq!(b.cells[4][3], 2);
        for mv in ["4", "4", "5", "5"] {
            assert!(b.play(mv));
        }
        assert_eq!(b.outcome, None);
        assert!(b.play("6"));
        assert_eq!(b.outcome, Some(Outcome::Win(1)));
        // a full column refuses the click
        let mut f = Board::new(Game::ConnectFour);
        for _ in 0..6 {
            assert!(f.play("0"));
        }
        assert_eq!(f.move_for_click(0, 0), None);
        assert!(!f.play("0"));
        // diagonal
        let mut g = Board::new(Game::ConnectFour);
        for mv in ["0", "1", "1", "2", "2", "3", "2", "3", "3", "6", "3"] {
            assert!(g.play(mv));
        }
        assert_eq!(g.outcome, Some(Outcome::Win(1)));
    }
}
