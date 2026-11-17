/*
 * doctor_sdk.h - SDK of native mods of The Godfather II (PC 1.0.789.0).
 *
 * A native mod is a 32-bit DLL with the extension .asi in the scripts folder
 * of the game, next to doctor_platform.asi. It exports
 *
 *     int doctor_mod_init(const DoctorApi *api);
 *
 * The platform module calls it once, on the main thread of the game, right
 * after the engine has registered its own classes. The mod checks api->abi
 * and api->size, keeps the pointer (it stays valid while the game runs) and
 * registers its classes, Lua functions, listeners and hooks. It returns
 * DOCTOR_OK or an error; the platform writes the result into
 * doctor_platform.log.
 *
 * Everything is cdecl. The same interface for Rust is the crate
 * doctor-toolkit-sdk (src/abi.rs); a test compares it with this file. What
 * is promised to stay: docs/analysis/asi-platform.md, section 7.
 *
 *  - The table DoctorApi only grows: new functions go to its end and
 *    api->size grows. A mod that wants a function added later checks size.
 *  - DOCTOR_ABI_VERSION changes only when something existing changes; a mod
 *    must refuse a platform of another version.
 *  - Unless said otherwise, call the functions on the main thread of the
 *    game: from doctor_mod_init and from the callbacks the platform makes.
 */
#ifndef DOCTOR_SDK_H
#define DOCTOR_SDK_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define DOCTOR_ABI_VERSION 1

/* Results. */
#define DOCTOR_OK 0
#define DOCTOR_ERR_ARGUMENT (-1) /* a null pointer, an empty name, a bad length */
#define DOCTOR_ERR_STATE (-2)    /* not now, or not an object of the platform */
#define DOCTOR_ERR_TAKEN (-3)    /* the name or the place is taken */
#define DOCTOR_ERR_FULL (-4)     /* a table of the platform is full */
#define DOCTOR_ERR_ENGINE (-5)   /* the game refused, memory not writable */

/* Lua types, as lua_type returns them (Lua 5.0). */
#define DOCTOR_LUA_TNONE (-1)
#define DOCTOR_LUA_TNIL 0
#define DOCTOR_LUA_TBOOLEAN 1
#define DOCTOR_LUA_TLIGHTUSERDATA 2
#define DOCTOR_LUA_TNUMBER 3
#define DOCTOR_LUA_TSTRING 4
#define DOCTOR_LUA_TTABLE 5
#define DOCTOR_LUA_TFUNCTION 6
#define DOCTOR_LUA_TUSERDATA 7

/* The priority nearly every subscription of the game has; a listener with a
 * higher number is called earlier. */
#define DOCTOR_PRIORITY_DEFAULT 0x8000

/* An object of the game (a Base of the engine) and a Lua state: opaque. */
typedef struct DoctorObject DoctorObject;
typedef struct DoctorLuaState DoctorLuaState;

/* One attribute of the packet of a class, as the reader of an object gets
 * it. Every attribute of the packet is given in order, also the ones the
 * object does not store: those come with present == 0 and a zero value. */
typedef struct DoctorAttr {
    uint32_t index;   /* number of the attribute in the packet */
    uint32_t present; /* 1 when the object stores a value */
    uint32_t raw;     /* an integer, the bits of a float, a hash */
    const void *ptr;  /* string, GUID, vector, matrix: the data; NULL when
                         not stored; valid only during the call */
} DoctorAttr;

/* A message on the bus, as the engine keeps it while sending. */
typedef struct DoctorMessage {
    uint32_t hash;      /* sdbm(lower(name)) */
    uint32_t data;      /* 0 or a pointer to the sender's data object */
    uint8_t delivering; /* set by the bus during delivery */
} DoctorMessage;

/* A new class of SimGroup objects. The structure and the name must stay
 * alive for as long as the game runs. Any callback may be NULL. */
typedef struct DoctorClass {
    uint32_t size;      /* sizeof(DoctorClass) */
    const char *name;   /* class name; its attributes are the packet of the same name */
    uint32_t data_size; /* bytes of the mod's data in every object, zeroed */
    /* the object exists, nothing is read yet */
    void (*create)(DoctorObject *object, void *data);
    /* one attribute of the packet of the class */
    void (*attribute)(DoctorObject *object, void *data, const DoctorAttr *attr);
    /* every attribute has been given */
    void (*loaded)(DoctorObject *object, void *data);
    /* a message the object listens to */
    void (*message)(DoctorObject *object, void *data, uint32_t msg, uint32_t word);
    /* the object is being destroyed; its subscriptions end after the call */
    void (*destroy)(DoctorObject *object, void *data);
} DoctorClass;

/* A listener of a mod that is not an object. */
typedef void (*DoctorMessageFn)(void *user, uint32_t msg, uint32_t word);
/* A filter of sends to all listeners: 0 lets the message go on, anything
 * else swallows it. It may change message->data. */
typedef uint32_t (*DoctorFilterFn)(void *user, DoctorMessage *message, uint32_t force);
/* A function of Lua: returns the number of its results. */
typedef int (*DoctorLuaFn)(DoctorLuaState *state);

typedef struct DoctorApi {
    uint32_t size; /* size of the table of this platform, bytes */
    uint32_t abi;  /* DOCTOR_ABI_VERSION of this platform */

    /* Name of the build of the game. */
    const char *(*edition)(void);
    /* One line into doctor_platform.log. Any thread. */
    void (*log)(const char *text);
    /* sdbm(lower(name)): the hash of a message, a class, a name. Any thread. */
    uint32_t (*hash)(const char *name);

    /* Registers a class the way the engine registers its own. Only from
     * doctor_mod_init. */
    int (*register_class)(const DoctorClass *cls);
    /* The mod's data of an object of one of its classes; NULL otherwise. */
    void *(*object_data)(DoctorObject *object);
    /* The 16 bytes of the GUID of an object. */
    int (*object_guid)(DoctorObject *object, uint8_t *out);
    /* Subscribes an object of a mod class to a message; the subscription
     * ends with the object. */
    int (*object_listen)(DoctorObject *object, uint32_t msg, uint32_t priority);
    int (*object_unlisten)(DoctorObject *object, uint32_t msg);
    /* Switches the listener of an object on (1) or off (0). */
    int (*object_enable)(DoctorObject *object, uint32_t on);

    /* Sends a message to every listener; 1 when somebody listened. */
    int (*send)(uint32_t msg, uint32_t word, uint32_t force);
    /* The same with data that points to an object. */
    int (*send_object)(uint32_t msg, DoctorObject *about, uint32_t force);
    /* Delivers a message to one object only. */
    int (*send_to)(uint32_t msg, uint32_t word, DoctorObject *to, uint32_t force);
    /* Sends to every listener after seconds of game time; about may be NULL. */
    int (*send_later)(uint32_t msg, float seconds, DoctorObject *about);
    /* The object a data word points to, or NULL. */
    DoctorObject *(*data_object)(uint32_t word);
    /* Subscribes a function; an identifier above 0 or an error. */
    int (*subscribe)(uint32_t msg, uint32_t priority, DoctorMessageFn listener, void *user);
    int (*unsubscribe)(int id);
    /* Adds a filter of sends to all listeners. */
    int (*add_filter)(DoctorFilterFn filter, void *user);

    /* Registers a global function of Lua in every state of the game, also
     * in the ones made later. The name must stay alive. */
    int (*register_lua)(const char *name, DoctorLuaFn function);
    /* Arguments of the running function: index from 1. Numbers of the
     * game's Lua are floats. */
    int (*lua_gettop)(DoctorLuaState *state);
    int (*lua_type)(DoctorLuaState *state, int index);
    float (*lua_tonumber)(DoctorLuaState *state, int index);
    int (*lua_toboolean)(DoctorLuaState *state, int index);
    const char *(*lua_tostring)(DoctorLuaState *state, int index);
    void *(*lua_touserdata)(DoctorLuaState *state, int index);
    void (*lua_pushnil)(DoctorLuaState *state);
    void (*lua_pushnumber)(DoctorLuaState *state, float value);
    void (*lua_pushboolean)(DoctorLuaState *state, int value);
    void (*lua_pushstring)(DoctorLuaState *state, const char *text);
    void (*lua_pushlightuserdata)(DoctorLuaState *state, void *pointer);

    /* Whether a mod may patch len bytes of code at address. Any thread. */
    int (*hook_check)(uint32_t address, uint32_t len);
    /* Replaces len (5 to 16) bytes of whole instructions at address with a
     * jump to detour and gives a trampoline that runs them and goes on after
     * them. No relative addresses in them, except that the first may be a
     * call or jmp with a 32-bit offset. */
    int (*hook_jump)(uint32_t address, uint32_t len, const void *detour, const void **trampoline);
    /* Points the call at address (5 bytes, 32-bit offset) to target and
     * gives what it called. */
    int (*hook_call)(uint32_t address, const void *target, const void **original);
} DoctorApi;

/* The entry of a mod. */
#ifdef _WIN32
__declspec(dllexport)
#endif
int doctor_mod_init(const DoctorApi *api);

/* sdbm(lower(name)), for constants computed by the mod itself. */
static inline uint32_t doctor_hash(const char *name) {
    uint32_t h = 0;
    for (; *name; name++) {
        uint32_t c = (uint8_t)*name;
        if (c >= 'A' && c <= 'Z')
            c += 32;
        h = c + (h << 6) + (h << 16) - h;
    }
    return h;
}

#if defined(__cplusplus)
#define DOCTOR_STATIC_ASSERT(cond, text) static_assert(cond, text)
#else
#define DOCTOR_STATIC_ASSERT(cond, text) _Static_assert(cond, text)
#endif
#if UINTPTR_MAX == 0xFFFFFFFFu
/* The layout of the 32-bit build, the only one the game has. */
DOCTOR_STATIC_ASSERT(sizeof(DoctorApi) == 136, "DoctorApi");
DOCTOR_STATIC_ASSERT(sizeof(DoctorClass) == 32, "DoctorClass");
DOCTOR_STATIC_ASSERT(sizeof(DoctorAttr) == 16, "DoctorAttr");
DOCTOR_STATIC_ASSERT(sizeof(DoctorMessage) == 12, "DoctorMessage");
#endif

#ifdef __cplusplus
}
#endif

#endif /* DOCTOR_SDK_H */
