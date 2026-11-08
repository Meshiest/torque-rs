/*
 * A native extension in C: one console function, C::twice(text), over the engine's C ABI (torque_plugin.h). It uses no C
 * library, so it builds as a shared library with any C compiler:
 *
 *   cc -shared -fPIC -I crates/torque-plugin-abi/include -o libc_sample.so examples/native-extension/c/sample.c
 *   clang -shared -I crates/torque-plugin-abi/include -o c_sample.dll examples/native-extension/c/sample.c
 *
 * Put the library in the game's plugins/ folder with a manifest, c_sample.toml:
 *
 *   id = "example.c-sample"
 *   version = "0.1.0"
 */

#include "torque_plugin.h"

/* The engine's function table: valid while the library is loaded. */
static const TorqueApi *api;

static size_t length(const char *s) {
    size_t n = 0;
    while (s[n] != 0) {
        n++;
    }
    return n;
}

static TorqueStr text(const char *s) {
    TorqueStr t;
    t.ptr = (const uint8_t *)s;
    t.len = length(s);
    return t;
}

/* C::twice(text): the text twice. */
static TorqueStatus twice(void *user, TorqueCall call) {
    static uint8_t out[1024];
    TorqueStr arg;
    TorqueStatus status;
    size_t i;
    (void)user;
    status = api->arg_str(call, 0, &arg);
    if (status != TORQUE_OK) {
        return status;
    }
    if (arg.len > sizeof out / 2) {
        return api->return_error(call, text("the text is too long"));
    }
    for (i = 0; i < arg.len; i++) {
        out[i] = arg.ptr[i];
        out[arg.len + i] = arg.ptr[i];
    }
    {
        TorqueStr result;
        result.ptr = out;
        result.len = 2 * arg.len;
        return api->return_str(call, result);
    }
}

TORQUE_EXPORT const uint32_t torque_plugin_abi_version = TORQUE_ABI_VERSION;

TORQUE_EXPORT TorqueStatus torque_plugin_init(const TorqueApi *engine, TorquePluginInfo *info) {
    TorqueArg args[1];
    TorqueNativeDecl decl;
    if (engine == 0 || info == 0) {
        return TORQUE_ERR_INVALID;
    }
    /* The first two fields are the same in every version: check them before anything else. */
    if (engine->abi_version != TORQUE_ABI_VERSION || engine->size < sizeof(TorqueApi)) {
        return TORQUE_ERR_ABI;
    }
    api = engine;

    info->abi_version = TORQUE_ABI_VERSION;
    info->id = text("example.c-sample");
    info->version = text("0.1.0");
    info->net_version = 0;
    info->has_net_version = 0;

    args[0].name = text("text");
    args[0].kind = TORQUE_ARG_REQUIRED;

    decl.side = TORQUE_SIDE_BOTH;
    decl.ns = text("C");
    decl.name = text("twice");
    decl.args = args;
    decl.arg_count = 1;
    decl.description = text("the text twice");
    decl.usage = text("");
    decl.docs = text("");
    decl.flags = 0;
    decl.call = twice;
    decl.user = 0;
    return api->register_function(api->init, &decl);
}
