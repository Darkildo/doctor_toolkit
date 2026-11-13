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

// --- Input ------------------------------------------------------------------

/// Controller manager (pointer); `+0x14`: the port (byte) of the controller
/// that plays — `0x0F` the keyboard and mouse, below `0x10` a device,
/// `0x12`/`0x13` none (`0x0068CBC0`, `0x0068C7A0`, `0x0068C7E0`).
pub const CONTROLLER_MANAGER: u32 = 0x0112_9814;
pub const CONTROLLER_PORT: u32 = 0x14;
/// Port (`u32`) the player reads his input from. Only the constructor of
/// `Player` sets it, from the controller manager (`0x007A2A5A`): below
/// `0x10` that port, else 0; `0x12` in network play. A start with `-stream`
/// makes the player before the manager has a port (the update of
/// `GameMainLoop` gives it one when the level is loaded, `0x0067D1ED`), so
/// the player listens to port 0 and the keyboard (15) does nothing.
pub const PLAYER_INPUT_PORT: u32 = 0x2498;
/// Input manager (pointer): per port a record of `0x18` bytes from `+0x14`,
/// whose first byte says the port is in use; the constructor of `Player`
/// sets it with `0x00444D20(port, 1)`.
pub const INPUT_MANAGER: u32 = 0x0122_3430;
pub const INPUT_PORT_RECORDS: u32 = 0x14;
pub const INPUT_PORT_RECORD_SIZE: u32 = 0x18;
/// Action maps of the ports (pointer): per port a block of `0x168` bytes;
/// `+0x10` the number of maps on the port's stack, `+0x14` the stack (map
/// pointers, the top last). The player asks for an action on his port
/// (`0x0079E920` → `0x00465700`, `0x00465660`), so his maps must be on it.
/// A map: `+8` count of users (byte), `+9` its name (`0x00464930`), e.g.
/// `basic_aiming_player`.
pub const ACTION_MAPS: u32 = 0x0122_3464;
pub const ACTION_MAPS_PORT_SIZE: u32 = 0x168;
pub const ACTION_MAPS_COUNT: u32 = 0x10;
pub const ACTION_MAPS_STACK: u32 = 0x14;
pub const ACTION_MAP_NAME: u32 = 9;
/// `bool Push(this, u8 port, const char* name, bool)` — `thiscall`, callee
/// pops 12 bytes: puts the map of that name on the port (`0x0079B5EE`, the
/// player's message handler, pushes with his port).
pub const ACTION_MAP_PUSH: u32 = 0x0046_4960;
/// `Pop(this, u8 port)` — `thiscall`, callee pops 4 bytes: takes the top map
/// off the port (`0x007A7485`, the destructor of `Player`).
pub const ACTION_MAP_POP: u32 = 0x0046_4A10;

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

// --- Journal (stage 49) ------------------------------------------------------

/// Import slot of `OutputDebugStringA` (`KERNEL32`). Every debug print of
/// the game goes through it: the printf of `0x0065BC40` (memory and timer
/// statistics, its own listeners at `0x01205A54`), FESL, asserts
/// (`0x006458F0`). The module puts its function into the slot and calls
/// what was there.
pub const IAT_OUTPUT_DEBUG_STRING: u32 = 0x00D5_50C4;
/// Lua `print` of the game (`0x0046BCE0`, not the stock one at
/// `0x0062D560`): it joins the arguments, finds the script and line of its
/// caller (`lua_getinfo "Sl"`) and calls the function in this pointer with
/// `(script, line, what, text)`, `cdecl`. In the release build it holds an
/// empty function (`0x0046BFC0`, `ret`; no code writes the pointer), so
/// prints went nowhere; the module puts its own there and calls that one
/// after it.
pub const LUA_PRINT_CALLBACK: u32 = 0x0110_A6C0;
/// `luaD_rawrunprotected(L, f, ud)` — `cdecl`, returns Lua's status. It runs
/// every protected call: `lua_pcall` and the game's calls (`0x00636570`),
/// the parser (`0x006366F0`) and `lua_resume` (`0x00636440`). When it
/// returns an error, the message is on top of the stack and the call stack
/// (`L->ci`) is not unwound yet: the module reads both there.
pub const LUA_RAW_RUN_PROTECTED: u32 = 0x0063_5AB0;
/// `luaG_errormsg` (`L` in `ESI`): every run time error passes here
/// (`lua_error` `0x00627BD0`, `luaG_runerror`) while the interpreter that
/// raised it still runs; then it calls the error handler and throws
/// (`0x00635A80`). The module jumps from its first five bytes
/// (`mov eax,[esi+0x54]; test eax,eax`) to read the call stack with the
/// current lines, and goes on at [`LUA_ERRORMSG_RESUME`].
pub const LUA_ERRORMSG: u32 = 0x0063_38A0;
pub const LUA_ERRORMSG_RESUME: u32 = 0x0063_38A5;
/// The calls of [`LUA_RAW_RUN_PROTECTED`] the module hooks.
pub const LUA_PROTECTED_CALLS: [u32; 3] = [0x0063_65A0, 0x0063_6722, 0x0063_6488];
/// `lua_State` of Lua 5.0 (`0x00636570`, `0x00632AA0`): stack top, first
/// slot, current `CallInfo`, the first `CallInfo`.
pub const LUA_TOP: u32 = 0x08;
pub const LUA_STACK: u32 = 0x1C;
pub const LUA_CI: u32 = 0x14;
pub const LUA_BASE_CI: u32 = 0x28;
/// `CallInfo` (0x18 bytes): `base` `+0`, `state` `+8` (bit 0: a C
/// function, bit 1: its `pc` is a local of the running interpreter),
/// `savedpc` `+0xC`, `pc` `+0x10`, `tailcalls` `+0x14` (`0x00632AA0`).
pub const LUA_CI_SIZE: u32 = 0x18;
pub const LUA_CI_STATE: u32 = 0x08;
pub const LUA_CI_SAVEDPC: u32 = 0x0C;
pub const LUA_CI_PC: u32 = 0x10;
pub const LUA_CI_TAILCALLS: u32 = 0x14;
/// Closure: `isC` byte `+6`, `Proto*` (Lua) or the C function at `+0xC`
/// (`0x00632D40`).
pub const LUA_CLOSURE_IS_C: u32 = 0x06;
pub const LUA_CLOSURE_PROTO: u32 = 0x0C;
/// `Proto`: code `+0xC`, line of each instruction `+0x14`, source name
/// (`TString*`) `+0x20`, sizes of code and lines `+0x2C`, `+0x30`, line
/// where the function starts `+0x3C` (`0x00632AA0`, `0x00632D40`).
pub const LUA_PROTO_CODE: u32 = 0x0C;
pub const LUA_PROTO_LINEINFO: u32 = 0x14;
pub const LUA_PROTO_SOURCE: u32 = 0x20;
pub const LUA_PROTO_SIZECODE: u32 = 0x2C;
pub const LUA_PROTO_SIZELINEINFO: u32 = 0x30;
pub const LUA_PROTO_LINEDEFINED: u32 = 0x3C;
/// `TString`: length `+0xC`, text `+0x10`.
pub const LUA_STRING_LEN: u32 = 0x0C;
pub const LUA_STRING_TEXT: u32 = 0x10;
/// Where the bus continues after the five bytes the module's jump takes
/// (`push ecx; mov edx,[esp+8]` at [`MESSAGE_SEND`]).
pub const MESSAGE_SEND_RESUME: u32 = 0x0040_8A05;
/// Flags word of an object (`+0x14`); the SimManager sets `0x20000000` when
/// it registers it (`0x004467C0`).
pub const OBJECT_FLAGS: u32 = 0x14;
/// Flags of the object's message listener (`+0x3C` + 4): bit 0 — switched
/// on (`0x0046C590` sets `1 | 0x12`; the bus checks it at `0x00408A7B`).
pub const OBJECT_LISTENER_FLAGS: u32 = 0x40;
/// A loaded stream (record of the list at `StreamManager+0x6C`): its id at
/// `+0x1C` is the group of every object it made (`0x00446E9D`,
/// `0x00405F40`).
pub const STREAM_ID: u32 = 0x1C;

// --- Stage 35: live debugging ---------------------------------------------------------

/// The variable holding the hash of `iMsgBeforeRender` (registered at start
/// up): the drawing of a level frame (`0x0040E1F0`) sends it through the bus
/// first, before it picks the view and records the commands of the frame.
/// The free camera of the module sets the view in its bus hook then.
pub const BEFORE_RENDER_HASH: u32 = 0x0120_6960;
/// The drawing of a level frame: `call 0x00408A00` with `iMsgBeforeRender`
/// (`0x0040E222`), then the view `*(*0x0122339C + 0x14)` (or slot 0 of TLS
/// `+0x24`).
pub const FRAME_DRAW: u32 = 0x0040_E1F0;
/// Sets the world matrix of a view: `EAX` the view, `ESI` the matrix (16
/// floats aligned on 16 — `MOVAPS`), on the stack `bool invert` (the callee
/// pops it, `RET 4`). Copies it to `+0x40`, makes the inverse (`+0`,
/// `0x0056C640`) and the planes (`+0x80`, `0x00423900`) and, on the main
/// thread, records the command that hands it to the render thread's copy
/// of the view (`0x00422C50`).
pub const VIEW_SET_MATRIX: u32 = 0x0042_2D40;
/// Textures by name hash: `*0x012058E0` — 4096 buckets by `hash & 0xFFF`;
/// a node is the texture object: name hash `+0x34`, next `+0x38`, handle
/// `+4` (lookup `0x00611000`).
pub const TEXTURE_MAP: u32 = 0x0120_58E0;
pub const TEXTURE_NODE_NAME: u32 = 0x34;
pub const TEXTURE_NODE_NEXT: u32 = 0x38;
pub const TEXTURE_NODE_HANDLE: u32 = 0x04;
/// The table of textures: 4096 entries of 0x38 bytes, a handle is the
/// index. Entry: `+0` state (2–3 made, 4 locked), `+1` levels, `+2` width,
/// `+4` height (u16), `+8` format (index of `0x00F15860`), `+0xC` flags (2
/// cube, 4 volume, 8 render target), `+0x14` the `IDirect3DBaseTexture9`,
/// `+0x30` pool (1 managed) — `0x00618640`, `0x00606AC0`.
pub const TEXTURE_TABLE: u32 = 0x011A_0F28;
pub const TEXTURE_ENTRY: u32 = 0x38;
/// The command "fill the levels of texture `handle` from `data`" of the
/// texture loader (`0x00619EF0` writes it, the render thread runs
/// `0x0061A5F0` → `0x00619F50` → `0x00606AC0`): four words in the frame's
/// command buffer — this pointer, the handle, the size, the data.
pub const TEXTURE_FILL_COMMAND: u32 = 0x0112_78A4;
/// The texture loader (`*0x01223558`): its list of uploads in flight `+0x20`
/// (entries of 0x14 bytes, handle at `+4`), their number `+0x24`
/// (`0x00619EA0`). A texture in that list is being loaded by the game: the
/// module leaves it alone.
pub const TEXTURE_LOADER: u32 = 0x0122_3558;
pub const TEXTURE_LOADER_PENDING: u32 = 0x20;
pub const TEXTURE_LOADER_PENDING_COUNT: u32 = 0x24;
