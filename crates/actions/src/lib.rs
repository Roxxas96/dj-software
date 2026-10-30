//! Every controllable behavior as a typed [`Action`] with a stable string id, the app-side
//! [`ControlState`] those actions update, and the observable state ([`StateKey`]) LEDs follow.
//!
//! [`apply`] turns an action and its [`Input`] into engine commands on the control thread.

mod action;
mod apply;
mod state;

pub use action::{Action, ActionKind, Input, Scope, Side, Target, ValueTarget};
pub use apply::{ApplyResult, apply};
pub use state::{ControlState, DeckControl, Led, SideControl, StateKey, led};
