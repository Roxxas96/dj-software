//! Every controllable behavior as a typed [`Action`] with a stable string id, the app-side
//! [`ControlState`] those actions update, and the observable state ([`StateKey`]) LEDs follow.
//!
//! [`apply`] turns an action and its [`Input`] into engine commands on the control thread,
//! or into an [`AppRequest`] for work only the app can do (such as editing a beatgrid).

mod action;
mod apply;
mod request;
mod state;
mod sync;

pub use action::{Action, ActionKind, Input, InputKind, Scope, Side, Target, ValueTarget};
pub use apply::{ApplyResult, GRID_SHIFT_FINE_MILLIS, GRID_SHIFT_MILLIS, apply};
pub use request::{AppRequest, BrowserRequest, GridEditRequest};
pub use state::{
    ControlState, DeckControl, HardwarePadMode, Led, PadMode, SideControl, StateKey, led,
};
