//! Inside a match's own server only (`--mods matchrun`, started by `mods/bare`), never the main server.
//!
//! The CPU rule: a bot starts on 20 CPU and gains 10 with each GCL level, up to 300. `cpuMaxPerTick` in the match's config
//! is 300 too, so a full bucket cannot push one tick past it either. `users.cpu` is what the runner reads, so the rule
//! only has to keep that field right, and it does so at the end of every tick. A bot starts with a full bucket: new accounts
//! have an empty one, and 20 CPU with nothing saved is 20 ms for the first tick, too short for a big bot to load, and a bot
//! that times out never saves any.
//!
//! A bot's code has no 5 MB limit here: a whole folder goes up, and `mods/bare` checks the one limit left when it arrives.
//!
//! `GET /api/matchrun/view` is what watching a run needs, in one reply: each player's numbers, who owns and who has reserved
//! which room, and the `map_view` of every room a player is in. `GET /api/matchrun/rooms?rooms=W1N1,W2N1` is everything in those rooms, for
//! a map close enough to draw it.

use backend::http::{obj, Request, Response};
use backend::mods::{Manifest, Mod, Registry};
use backend::{edit_user, field, obj_set, str_field, ApiCtx, Route};
use engine::constants::{GCL_MULTIPLY, GCL_POW};
use engine::doc::Val;
use engine::processor::global::GlobalCtx;
use engine::world::{UserIdx, UserState, World};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use util::json::Json;

const START_CPU: f64 = 20.0;
const CPU_PER_LEVEL: f64 = 10.0;
const MAX_CPU: f64 = 300.0;
const FULL_BUCKET: f64 = 10_000.0;

pub struct MatchRun;

/// What the server registers (modlist finds it by this name).
pub static MOD: MatchRun = MatchRun;

impl Mod for MatchRun {
    fn manifest(&self) -> Manifest {
        Manifest { name: "matchrun", version: env!("CARGO_PKG_VERSION"), dependencies: &[] }
    }

    fn register(&self, r: &mut Registry) {
        backend::api::user::MAX_CODE_UNITS.store(usize::MAX, Ordering::Relaxed);
        r.engine.processor.global_stage.push(hold_cpu);
        r.routes.push(Route { method: "GET", path: "/api/matchrun/view", handler: view });
        r.routes.push(Route { method: "GET", path: "/api/matchrun/rooms", handler: rooms });
    }
}

/// The game's own formula: `Game.gcl.level`.
fn gcl_level(points: f64) -> u32 {
    (points.max(0.0) / f64::from(GCL_MULTIPLY)).powf(1.0 / GCL_POW).floor() as u32 + 1
}

fn cpu_for(level: u32) -> f64 {
    (START_CPU + CPU_PER_LEVEL * f64::from(level.saturating_sub(1))).min(MAX_CPU)
}

fn number(doc: &engine::doc::Obj, key: &str) -> f64 {
    field(doc, key).and_then(Val::as_f64).unwrap_or(0.0)
}

/// A bot's CPU now, and its bucket if that changes too. GCL only goes up, so only a new account has more CPU than the rule gives.
fn held(cpu: f64, gcl: f64) -> (f64, Option<f64>) {
    let want = cpu_for(gcl_level(gcl));
    (want, (cpu > want).then_some(FULL_BUCKET))
}

/// A player's account: placing a spawn makes it active. Of a world's own users Screeps never is, and Invader and Source
/// Keeper run code like any bot but keep the world's limits.
fn plays(st: &UserState) -> bool {
    !matches!(st.id.as_str(), "2" | "3") && number(&st.doc, "active") > 0.0
}

fn hold_cpu(w: &mut World, _ctx: &GlobalCtx) {
    for u in 0..w.users.len() {
        if !plays(&w.users[u]) {
            continue;
        }
        let doc = &w.users[u].doc;
        let (want, bucket) = held(number(doc, "cpu"), number(doc, "gcl"));
        if number(doc, "cpu") != want {
            let mut next = (**doc).clone();
            obj_set(&mut next, "cpu", Val::Num(want));
            if let Some(full) = bucket {
                obj_set(&mut next, "cpuAvailable", Val::Num(full));
            }
            edit_user(w, u as UserIdx, |st| st.doc = Arc::new(next));
        }
    }
}

/// Who a controller is reserved for. The game clears the field as a reservation ends.
fn reserver(controller: &engine::doc::Obj) -> Option<&str> {
    match field(controller, "reservation") {
        Some(Val::Obj(reservation)) => str_field(reservation, "user"),
        _ => None,
    }
}

/// GET /api/matchrun/view
fn view(_req: &Request, ctx: &ApiCtx) -> Response {
    let w = ctx.published.get();
    let mut players = Vec::new();
    let mut ids = Vec::new();
    for st in w.users.iter().filter(|st| plays(st)) {
        let Some(name) = str_field(&st.doc, "username") else { continue };
        let points = number(&st.doc, "gcl");
        ids.push(st.id.as_str().to_string());
        players.push(obj(vec![
            ("id", Json::Str(st.id.as_str().to_string())),
            ("name", Json::Str(name.to_string())),
            ("gcl", Json::Num(f64::from(gcl_level(points)))),
            ("cpu", Json::Num(number(&st.doc, "cpu"))),
            ("used", Json::Num(number(&st.doc, "lastUsedCpu"))),
            ("bucket", Json::Num(number(&st.doc, "cpuAvailable"))),
        ]));
    }
    let (mut owned, mut reserved, mut rooms) = (Vec::new(), Vec::new(), Vec::new());
    for (i, room) in w.rooms.iter().enumerate() {
        let name = w.meta.rooms[i].name.to_string();
        for d in room.objects.iter().filter(|d| d.kind() == "controller") {
            if let Some(user) = str_field(&d.obj, "user") {
                owned.push(Json::Arr(vec![Json::Str(name.clone()), Json::Str(user.to_string()), Json::Num(number(&d.obj, "level"))]));
            } else if let Some(user) = reserver(&d.obj).filter(|user| ids.iter().any(|id| id == user)) {
                // Invaders reserve too, and that is no border anyone is holding
                reserved.push(Json::Arr(vec![Json::Str(name.clone()), Json::Str(user.to_string())]));
            }
        }
        if ids.iter().any(|id| room.map_view.contains(id.as_str())) {
            if let Ok(units) = Json::parse(&room.map_view) {
                rooms.push((name, units));
            }
        }
    }
    Response::ok(obj(vec![
        ("tick", Json::Num(w.time as f64)),
        ("players", Json::Arr(players)),
        ("owned", Json::Arr(owned)),
        ("reserved", Json::Arr(reserved)),
        ("rooms", Json::Obj(rooms)),
    ]))
}

/// GET /api/matchrun/rooms?rooms=W1N1,W2N1: `{tick, rooms: {W1N1: [objects]}}`. Built as text, as each object already is.
fn rooms(req: &Request, ctx: &ApiCtx) -> Response {
    let w = ctx.published.get();
    let mut out = format!("{{\"tick\":{},\"rooms\":{{", w.time);
    let mut first = true;
    for name in req.param("rooms").unwrap_or("").split(',') {
        let Some(i) = w.meta.rooms.iter().position(|m| &*m.name == name) else { continue };
        if !std::mem::take(&mut first) {
            out.push(',');
        }
        out += &format!("\"{name}\":[");
        for (n, doc) in w.rooms[i].objects.iter().enumerate() {
            if n > 0 {
                out.push(',');
            }
            out += doc.json();
        }
        out.push(']');
    }
    out += "}}";
    Response::text(200, "application/json; charset=utf-8", &out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gcl_levels_follow_the_game() {
        assert_eq!(gcl_level(0.0), 1);
        assert_eq!(gcl_level(999_999.0), 1);
        assert_eq!(gcl_level(1_000_000.0), 2);
        assert_eq!(gcl_level(5_278_031.0), 2);
        assert_eq!(gcl_level(5_278_032.0), 3);
    }

    #[test]
    fn only_a_new_account_gets_a_full_bucket() {
        assert_eq!(held(100.0, 0.0), (20.0, Some(FULL_BUCKET)), "the server's default for a new account");
        assert_eq!(held(20.0, 0.0), (20.0, None));
        assert_eq!(held(20.0, 1_000_000.0), (30.0, None), "a level up");
        assert_eq!(held(100.0, gcl_points(10)), (110.0, None), "nor when a bot reaches what a new account has");
    }

    fn gcl_points(level: u32) -> f64 {
        f64::from(level - 1).powf(GCL_POW) * f64::from(GCL_MULTIPLY) + 1.0
    }

    #[test]
    fn a_reservation_names_who_holds_it() {
        let mut controller = engine::doc::Obj::new();
        assert_eq!(reserver(&controller), None);
        obj_set(&mut controller, "reservation", Val::Null);
        assert_eq!(reserver(&controller), None, "how the game ends one");
        let mut reservation = engine::doc::Obj::new();
        obj_set(&mut reservation, "user", backend::s("7"));
        obj_set(&mut controller, "reservation", Val::Obj(reservation));
        assert_eq!(reserver(&controller), Some("7"));
    }

    #[test]
    fn a_bot_starts_on_20_and_gains_10_a_level_up_to_300() {
        assert_eq!(cpu_for(1), 20.0);
        assert_eq!(cpu_for(2), 30.0);
        assert_eq!(cpu_for(10), 110.0);
        assert_eq!(cpu_for(29), 300.0);
        assert_eq!(cpu_for(30), 300.0);
        assert_eq!(cpu_for(500), 300.0);
    }
}
