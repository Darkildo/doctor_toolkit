/*
 * doctor_example_c.asi: the smallest native mod in C (stage 80).
 *
 * It adds two Lua functions through doctor_platform.asi:
 *   DoctorCAdd(a, b)    the sum of two numbers
 *   DoctorCUpper(text)  the text in capital letters
 * and the class DoctorCEcho, whose objects send the message of attribute 1
 * each time the message of attribute 0 comes.
 *
 * Build (build.sh does it):
 *   i686-w64-mingw32-gcc -shared -O2 -static-libgcc \
 *       -I ../../../crates/doctor-toolkit-sdk/include \
 *       -o doctor_example_c.asi doctor_example_c.c
 */
#include "doctor_sdk.h"

static const DoctorApi *api;

static int lua_add(DoctorLuaState *L) {
    api->lua_pushnumber(L, api->lua_tonumber(L, 1) + api->lua_tonumber(L, 2));
    return 1;
}

static int lua_upper(DoctorLuaState *L) {
    char out[128];
    const char *text = api->lua_type(L, 1) == DOCTOR_LUA_TSTRING ? api->lua_tostring(L, 1) : 0;
    unsigned i = 0;
    if (!text) {
        api->lua_pushnil(L);
        return 1;
    }
    for (; text[i] && i < sizeof out - 1; i++)
        out[i] = text[i] >= 'a' && text[i] <= 'z' ? text[i] - 32 : text[i];
    out[i] = 0;
    api->lua_pushstring(L, out);
    return 1;
}

/* The data of one DoctorCEcho object. */
typedef struct Echo {
    uint32_t in, out;
} Echo;

static void echo_attribute(DoctorObject *object, void *data, const DoctorAttr *attr) {
    Echo *echo = data;
    /* An attribute that is not stored has present == 0 and ptr == NULL. */
    uint32_t msg = attr->present && attr->ptr ? api->hash(attr->ptr) : 0;
    if (attr->index == 0) {
        echo->in = msg;
        if (msg)
            api->object_listen(object, msg, DOCTOR_PRIORITY_DEFAULT);
    } else if (attr->index == 1) {
        echo->out = msg;
    }
}

static void echo_message(DoctorObject *object, void *data, uint32_t msg, uint32_t word) {
    Echo *echo = data;
    (void)word;
    if (msg == echo->in && echo->out)
        api->send_object(echo->out, object, 0);
}

static const DoctorClass echo_class = {
    sizeof(DoctorClass), "DoctorCEcho", sizeof(Echo), 0, echo_attribute, 0, echo_message, 0,
};

int doctor_mod_init(const DoctorApi *platform) {
    int result;
    if (!platform || platform->abi != DOCTOR_ABI_VERSION || platform->size < sizeof(DoctorApi))
        return DOCTOR_ERR_STATE;
    api = platform;
    if ((result = api->register_class(&echo_class)) != DOCTOR_OK)
        return result;
    if ((result = api->register_lua("DoctorCAdd", lua_add)) != DOCTOR_OK)
        return result;
    if ((result = api->register_lua("DoctorCUpper", lua_upper)) != DOCTOR_OK)
        return result;
    api->log("doctor_example_c: class DoctorCEcho, Lua DoctorCAdd and DoctorCUpper");
    return DOCTOR_OK;
}
