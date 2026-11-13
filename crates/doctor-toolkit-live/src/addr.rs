//! Addresses of `godfather2.exe` PC 1.0.789.0 used by the module.
//!
//! Found with Ghidra 12.2 (headless analysis of this executable) and checked
//! in the running game; the evidence for each is in
//! `docs/analysis/live-link.md` §2–§5. [`crate::edition::CHECKS`] holds the
//! bytes of the code that proves them: the module refuses a build where
//! they differ.

// --- Main loop -----------------------------------------------------------

/// `call 0x00439FA0` at the top of every pass of the main loop
/// (`0x0068AEB0`, before the current state runs its frame). The module
/// replaces the target of this call: its hook runs on the main thread,
/// between frames, where it reads and changes the game.
pub const FRAME_CALL: u32 = 0x0068_B04B;
/// The function that call reaches (`cdecl`, no arguments, `bool` in `AL`).
pub const FRAME_CALLEE: u32 = 0x0043_9FA0;
/// Dispatcher of main loop states (pointer); `+4`: the current state.
pub const LOOP_DISPATCHER: u32 = 0x0112_980C;
/// Vtables of the three states (`0x0068AEB0`, constructors `0x0067C390`,
/// `0x0067C590`).
pub const VTABLE_INIT_LOOP: u32 = 0x00D5_A1F8;
pub const VTABLE_FRONTEND_LOOP: u32 = 0x00D5_7758;
pub const VTABLE_GAME_LOOP: u32 = 0x00D5_7728;
/// Slot `+0x2C` of the `GameMainLoop` vtable: its update, called once a
/// frame by its method `+0xC` (`0x0067C290`, run by `0x00683450`) between
/// the start of the frame (`0x0068BAF0(1)`) and its end (`0x0068BCD0(1)`);
/// it returns the state's status. The module puts its second hook here:
/// writes run inside the frame, where the game's own messages and teleports
/// run.
pub const GAME_LOOP_UPDATE_SLOT: u32 = 0x00D5_7754;
/// What the slot holds: the update of `GameMainLoop`.
pub const GAME_LOOP_UPDATE: u32 = 0x0067_D160;
/// The render command buffer of the frame (pointer). Set only inside a
/// frame; screen fades of a teleport write into it (`0x0040C1F0`).
pub const FRAME_COMMANDS: u32 = 0x0120_6880;

// --- Player ---------------------------------------------------------------

/// Player list (pointer): `+4` array of pointers, `+0xC` count. Each entry
/// points `0x1F30` bytes into its `Player` (`0x007DD044`), and its virtual
/// method `+0xC` returns the player (`0x0040E0A0`, `0x00469230`).
pub const PLAYER_LIST: u32 = 0x0122_33A0;
/// Offset of the list entry inside `Player`.
pub const PLAYER_ENTRY_OFFSET: u32 = 0x1F30;
/// World matrix of an `Entity` (and of `Player`): rows right, up, forward,
/// translation (`0x00471610`, constructor `0x0046E6B0`).
pub const ENTITY_MATRIX: u32 = 0xD0;
/// Player state word; bit 7 set: the game does not teleport him now
/// (`0x007DD050`).
pub const PLAYER_STATE: u32 = 0x24A0;
/// Virtual method `+0x14C` of `Player` called before a teleport with
/// `(0x46F5C7B4, 1, 0)` (`0x007DD062`).
pub const PLAYER_PREPARE_METHOD: u32 = 0x14C;
pub const PLAYER_PREPARE_ARG: u32 = 0x46F5_C7B4;
/// `Player::Teleport(this, const Matrix*, flags, MessageHandle* done,
/// Partitions*)` — `thiscall`, callee pops 16 bytes; what
/// `PlayerTeleportRelay` calls (`0x007DD117`).
pub const PLAYER_TELEPORT: u32 = 0x0079_E3C0;

// --- Camera ---------------------------------------------------------------

/// Camera manager (pointer). When `+0x154` is not zero, `+0x14` is the
/// camera the game renders from, else the one of the thread's TLS slot 0 at
/// `+0x24` (`0x0040E0A0`, which uses it for the sound listener when the
/// byte `0x01205248` is set). In a level it is the static camera object
/// `0x01207DB0`.
pub const CAMERA_MANAGER: u32 = 0x0122_339C;
pub const CAMERA_CURRENT: u32 = 0x14;
pub const CAMERA_FLAG: u32 = 0x154;
/// The camera's world matrix (translation `+0x70`); `+0` holds its inverse.
pub const CAMERA_MATRIX: u32 = 0x40;
/// Six frustum planes (normal, distance) after the matrices.
pub const CAMERA_PLANES: u32 = 0x80;

// --- SimGroup objects ------------------------------------------------------

/// SimManager (pointer): `+0x48` buckets and `+0x4C` bucket count of the
/// table of classes; each class record: `[0]` class hash, `[4]` first
/// object, `[6]` next record of the bucket; objects of a class are chained
/// through `+8` (`0x004467C0`).
pub const SIM_MANAGER: u32 = 0x0122_3410;
pub const SIM_CLASS_BUCKETS: u32 = 0x48;
pub const SIM_CLASS_BUCKET_COUNT: u32 = 0x4C;
/// Object fields (`0x0046C590`, `0x004467C0`): the key of the table above
/// (a small group number, not the class) and the GUID.
pub const OBJECT_GROUP: u32 = 0x1C;
pub const OBJECT_GUID: u32 = 0x20;
/// Registry of SimGroup classes (`0x004833A0`, `0x00483720`): pointer to
/// a sorted array, `+4` count; an entry is `{hash, factory, name, size}`.
pub const CLASS_REGISTRY: u32 = 0x0122_39BC;
/// The message listener inside every object (`Base` `+0x3C`): the bus calls
/// its virtual method `+4` with the message (`0x00408A00`, `0x007DD130`).
pub const OBJECT_LISTENER: u32 = 0x3C;
/// Virtual method `+0x10` of an object: `bool As(u32 class, void** out)`
/// (`0x007DCCD0`).
pub const OBJECT_AS_METHOD: u32 = 0x10;
/// Hash of `Entity` (objects with a world matrix).
pub const CLASS_ENTITY: u32 = 0x3852_3FC3;

// --- Messages ---------------------------------------------------------------

/// Table of messages (pointer to 4096 buckets, `hash & 0xFFF`); an entry:
/// `[0]` next, `[1]` listener blocks, `[3]` pending removals, `[4]` hash,
/// `+0x16` flags (`0x00408A00`).
pub const MESSAGE_TABLE: u32 = 0x0116_2380;
/// `bool Send(Message* m, bool force)` — `cdecl`; a message is
/// `{u32 hash, u32 data, u8 sent}` (`0x004C82C0`, Lua `SendMessage`).
pub const MESSAGE_SEND: u32 = 0x0040_8A00;

// --- Lua -----------------------------------------------------------------

/// Lua manager (pointer); `+0x18` is the `lua_State` of the game's scripts
/// (`0x00480D90`).
pub const LUA_MANAGER: u32 = 0x0122_33F4;
pub const LUA_MANAGER_STATE: u32 = 0x18;
/// `int load(lua_State* L, size_t size)` with the buffer in `ECX` and the
/// chunk name in `EAX` — `luaL_loadbuffer` (`0x004CB0C0`, `0x0062E4C0`).
pub const LUA_LOAD_BUFFER: u32 = 0x0062_9280;
/// `int luaD_pcall(Pfunc f, void* ud, ptrdiff_t old_top)` with `L` in
/// `ESI` and the error function in `EDX`, `cdecl` (`0x006293C0`).
pub const LUA_PCALL: u32 = 0x0063_6570;
/// `f_call` of `lua_pcall`: `ud` is `{StkId func; int nresults}`.
pub const LUA_F_CALL: u32 = 0x0062_61E0;

// --- Time and streams ---------------------------------------------------------

/// Simulation time in milliseconds (`u32`; Lua `GetSimTime`, `0x0046A850`).
pub const SIM_TIME_MS: u32 = 0x0120_5224;
/// The `TimeOfDayManager` object (pointer, a SimGroup object).
pub const TIME_OF_DAY: u32 = 0x0112_A53C;
/// Hours, minutes, seconds (`u32` each) of the time of day.
pub const TIME_OF_DAY_HMS: u32 = 0x64;
/// StreamManager (pointer); `+0x6C` list of loaded streams, next at `+4`,
/// name pointer `+8`, name length `+0xC` (`0x00405F40`).
pub const STREAM_MANAGER: u32 = 0x0122_337C;
pub const STREAM_LIST: u32 = 0x6C;

/// Code of the executable: calls and vtables must point here.
pub const TEXT_START: u32 = 0x0040_1000;
pub const TEXT_END: u32 = 0x00D5_435A;
