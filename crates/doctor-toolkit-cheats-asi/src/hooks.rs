//! The two hooks of the mod (stage 121), through the platform's
//! `hook_jump`/`hook_call` (the registry of places refuses one the toolkit
//! uses); installed from `doctor_mod_init`, after the mod has checked the
//! bytes it expects.
//!
//! - No reload: `add dword ptr [esi+0x328], -1` of `Gun::Fire`
//!   (`0x0070334A`, 7 bytes) is skipped when `esi` is the gun the tick has
//!   put into [`NO_RELOAD_GUN`] (the player's, while the item is on); any
//!   other gun runs it.
//! - Damage of enemies: the call of `Character::OnDamage(DamageInfo*)` at
//!   the end of `Player::OnDamage` (`0x0079D1E2`) goes through
//!   [`on_damage`], which multiplies the damage by [`ENEMY_DAMAGE`] unless
//!   the player hit himself.

use std::arch::global_asm;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

use doctor_sdk::Platform;
use doctor_sdk::abi;

use crate::game;

/// The gun whose magazine does not run down; 0 — none.
pub static NO_RELOAD_GUN: AtomicU32 = AtomicU32::new(0);
static NO_RELOAD_TRAMPOLINE: AtomicU32 = AtomicU32::new(0);
/// `f32` bits of the multiplier of the damage enemies do.
pub static ENEMY_DAMAGE: AtomicU32 = AtomicU32::new(0x3F80_0000);
/// The handle of the player (`Player + 0x48`) as `DamageInfo` names who hit.
pub static PLAYER_HANDLE: AtomicU32 = AtomicU32::new(0);
static ON_DAMAGE_ORIGINAL: AtomicU32 = AtomicU32::new(0);

global_asm!(
    ".globl _doctor_cheats_no_reload",
    "_doctor_cheats_no_reload:",
    "cmp esi, dword ptr [{gun}]",
    "je 2f",
    "jmp dword ptr [{trampoline}]",
    "2:",
    "push {resume}",
    "ret",
    gun = sym NO_RELOAD_GUN,
    trampoline = sym NO_RELOAD_TRAMPOLINE,
    resume = const game::NO_RELOAD_RESUME,
);

unsafe extern "C" {
    fn doctor_cheats_no_reload();
}

/// What the call at `0x0079D1E2` reaches: `Character::OnDamage` after the
/// multiplier.
unsafe extern "thiscall" fn on_damage(this: u32, info: *mut u8) -> u32 {
    let k = f32::from_bits(ENEMY_DAMAGE.load(Ordering::Relaxed));
    if k != 1.0 && k.is_finite() && !info.is_null() {
        // SAFETY: the `DamageInfo` the game passes.
        unsafe {
            let by = *(info.add(game::DAMAGE_BY as usize) as *const u32);
            if by != PLAYER_HANDLE.load(Ordering::Relaxed) {
                let amount = info.add(game::DAMAGE_AMOUNT as usize) as *mut f32;
                *amount *= k.max(0.0);
            }
        }
    }
    // SAFETY: the function the call reached before (checked bytes).
    unsafe {
        let original: unsafe extern "thiscall" fn(u32, *mut u8) -> u32 =
            std::mem::transmute(ON_DAMAGE_ORIGINAL.load(Ordering::Relaxed) as usize);
        original(this, info)
    }
}

/// Installs both; for each — whether it is in place.
pub fn install(p: &Platform) -> (bool, bool) {
    let api = p.api();
    let no_reload = game::code_is(game::NO_RELOAD_AT, &game::NO_RELOAD_BYTES) && {
        let mut trampoline: *const c_void = std::ptr::null();
        // SAFETY: seven bytes of one whole instruction without relative
        // addresses (checked above); the detour keeps every register.
        let r = unsafe {
            (api.hook_jump)(
                game::NO_RELOAD_AT,
                game::NO_RELOAD_BYTES.len() as u32,
                doctor_cheats_no_reload as unsafe extern "C" fn() as *const c_void,
                &mut trampoline,
            )
        };
        NO_RELOAD_TRAMPOLINE.store(trampoline as usize as u32, Ordering::Relaxed);
        r == abi::OK && !trampoline.is_null()
    };
    let enemy = game::code_is(game::ENEMY_DAMAGE_AT, &game::ENEMY_DAMAGE_BYTES) && {
        let mut original: *const c_void = std::ptr::null();
        // SAFETY: a call of a thiscall function with one argument; the
        // wrapper has the same convention.
        let r = unsafe {
            (api.hook_call)(
                game::ENEMY_DAMAGE_AT,
                on_damage as unsafe extern "thiscall" fn(u32, *mut u8) -> u32 as *const c_void,
                &mut original,
            )
        };
        ON_DAMAGE_ORIGINAL.store(original as usize as u32, Ordering::Relaxed);
        r == abi::OK && !original.is_null()
    };
    (no_reload, enemy)
}
