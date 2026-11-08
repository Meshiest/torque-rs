#ifndef TORQUE_PLUGIN_H
#define TORQUE_PLUGIN_H

/* Generated from crates/torque-plugin-abi/src/lib.rs by `just c-header`: do not edit. */

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

/* How an extension exports its two symbols. */
#if defined(_WIN32)
#define TORQUE_EXPORT __declspec(dllexport)
#else
#define TORQUE_EXPORT __attribute__((visibility("default")))
#endif

// The version of this ABI. An extension reports the version it was built against, and the engine loads only its own.
#define TORQUE_ABI_VERSION 1

// The longest name, id, version or namespace the engine takes (bytes).
#define TORQUE_MAX_NAME 256

// The longest usage text, description, docs or log message the engine takes (bytes).
#define TORQUE_MAX_TEXT (64 * 1024)

// The longest script value (a return value, a global's value) the engine takes (bytes).
#define TORQUE_MAX_VALUE (1024 * 1024)

// The most script arguments a console function or method declares.
#define TORQUE_MAX_ARGS 32

// The most console functions, methods, globals and passes one extension registers.
#define TORQUE_MAX_REGISTRATIONS 1024

// The most vertices one pass draws in one frame (the rest are refused with [`TORQUE_ERR_RANGE`]).
#define TORQUE_MAX_PASS_VERTICES (1 << 20)

// Which script engines a registration goes into: every server's and every client's.
#define TORQUE_SIDE_BOTH 0

// Every server's script engine.
#define TORQUE_SIDE_SERVER 1

// Every client's script engine.
#define TORQUE_SIDE_CLIENT 2

// A script argument the script must pass.
#define TORQUE_ARG_REQUIRED 0

// A script argument the script may leave out: only after the required ones.
#define TORQUE_ARG_OPTIONAL 1

// Every argument left: the last one.
#define TORQUE_ARG_REST 2

// A development command: it runs in a debug build of the engine, or once `$Pref::Dev` is true.
#define TORQUE_NATIVE_DEV 1

// [`TorqueNativeDecl::usage`] is the whole usage text, kept as written (none is made from the arguments).
#define TORQUE_NATIVE_EXACT_USAGE 2

// A message for the console or the engine's log.
#define TORQUE_LOG_INFO 0

// A warning.
#define TORQUE_LOG_WARN 1

// An error.
#define TORQUE_LOG_ERROR 2

// The frame's stage after the opaque pass: world-space triangles go into the HDR frame, depth tested and written.
#define TORQUE_STAGE_OPAQUE 2

// The stage after the translucent passes: world-space triangles go into the HDR frame, depth tested and written.
#define TORQUE_STAGE_TRANSLUCENT 3

// The stage after the tonemap: triangles go over the finished picture, with no depth.
#define TORQUE_STAGE_OVERLAY 6

// Vertices in world units (the engine's z-up world), seen through [`TorquePassFrame::view_proj`].
#define TORQUE_SPACE_WORLD 0

// Vertices in clip space (x and y from -1 to 1 across the stage's viewport, y up; z is ignored at the overlay).
#define TORQUE_SPACE_CLIP 1

// What every function across the boundary returns: [`TORQUE_OK`] or one of the `TORQUE_ERR_` codes.
typedef uint32_t TorqueStatus;

// The handle of one `torque_plugin_init` call: what the `register_*` functions name. Live only during that call.
typedef uint64_t TorqueInit;

// Text: `len` bytes at `ptr`, not NUL-terminated. `ptr` may be null only when `len` is 0. Names, ids, versions and usage
// texts must be UTF-8; script values are bytes.
typedef struct TorqueStr {
    // The first byte.
    const uint8_t *ptr;
    // How many bytes.
    size_t len;
} TorqueStr;

// One script argument of a console function or method: its name in the usage text and its kind (`TORQUE_ARG_`).
typedef struct TorqueArg {
    // The name the usage text shows.
    struct TorqueStr name;
    // [`TORQUE_ARG_REQUIRED`], [`TORQUE_ARG_OPTIONAL`] or [`TORQUE_ARG_REST`].
    uint32_t kind;
} TorqueArg;

// The handle of one call of a console function or method: what the `arg_*`, `return_*` and global functions name. Live
// only during that call.
typedef uint64_t TorqueCall;

// A console function (`register_function`) or method (`register_method`) the extension declares.
//
// The engine copies everything it needs before the register call returns, except `user`, which it hands back to `call`.
typedef struct TorqueNativeDecl {
    // [`TORQUE_SIDE_BOTH`], [`TORQUE_SIDE_SERVER`] or [`TORQUE_SIDE_CLIENT`].
    uint32_t side;
    // A function's namespace (empty for the global one), or a method's class.
    struct TorqueStr ns;
    // The name scripts call.
    struct TorqueStr name;
    // The script arguments, `arg_count` of them (may be null when `arg_count` is 0).
    const struct TorqueArg *args;
    // How many script arguments.
    size_t arg_count;
    // The usage text's description, after the signature made from the arguments (empty for none).
    struct TorqueStr description;
    // With [`TORQUE_NATIVE_EXACT_USAGE`]: the whole usage text. Otherwise ignored.
    struct TorqueStr usage;
    // The whole doc comment, for the script reference (empty for none).
    struct TorqueStr docs;
    // [`TORQUE_NATIVE_DEV`] and [`TORQUE_NATIVE_EXACT_USAGE`], or 0.
    uint32_t flags;
    // The function the engine calls (a [`TorqueNativeFn`]).
    TorqueStatus (*call)(void *user, TorqueCall call);
    // Handed back to `call` as it is; never read by the engine.
    void *user;
} TorqueNativeDecl;

// `register_function` and `register_method`.
typedef TorqueStatus (*TorqueRegisterNativeFn)(TorqueInit init, const struct TorqueNativeDecl *decl);

// `register_global`.
typedef TorqueStatus (*TorqueRegisterGlobalFn)(TorqueInit init, uint32_t side, struct TorqueStr name, struct TorqueStr default_value);

// The handle of one pass's drawing in one frame: what [`TorqueApi::draw_triangles`] names. Live only during that call.
typedef uint64_t TorqueDraw;

// What a pass is told about the frame it draws in.
typedef struct TorquePassFrame {
    // The stage (`TORQUE_STAGE_`).
    uint32_t stage;
    // The size of the stage's target, in pixels.
    uint32_t size[2];
    // Where the world is in the target: left, top, width, height.
    uint32_t viewport[4];
    // The camera's view and projection, world to clip, column major.
    float view_proj[16];
    // The camera's position in the world.
    float eye[3];
    // The scene's effects time, in seconds.
    float time;
} TorquePassFrame;

// A render pass the extension declares (`register_render_pass`): each view of the world makes an instance with `create`
// and draws it at `stage` of every frame.
typedef struct TorquePassDecl {
    // [`TORQUE_STAGE_OPAQUE`], [`TORQUE_STAGE_TRANSLUCENT`] or [`TORQUE_STAGE_OVERLAY`].
    uint32_t stage;
    // Makes an instance (a [`TorquePassCreateFn`]).
    TorqueStatus (*create)(void *user, TorqueDraw draw, void **out);
    // Draws an instance (a [`TorquePassDrawFn`]).
    TorqueStatus (*draw)(void *pass, const struct TorquePassFrame *frame, TorqueDraw draw);
    // Frees an instance (a [`TorquePassDropFn`]; may be null).
    void (*drop)(void *pass);
    // Handed to `create` as it is; never read by the engine.
    void *user;
} TorquePassDecl;

// `register_render_pass`.
typedef TorqueStatus (*TorqueRegisterPassFn)(TorqueInit init, const struct TorquePassDecl *decl);

// `arg_count`.
typedef TorqueStatus (*TorqueArgCountFn)(TorqueCall call, uint32_t *out);

// `arg_str`.
typedef TorqueStatus (*TorqueArgStrFn)(TorqueCall call, uint32_t index, struct TorqueStr *out);

// `arg_int`.
typedef TorqueStatus (*TorqueArgIntFn)(TorqueCall call, uint32_t index, int32_t *out);

// `arg_float`.
typedef TorqueStatus (*TorqueArgFloatFn)(TorqueCall call, uint32_t index, float *out);

// `arg_bool`.
typedef TorqueStatus (*TorqueArgBoolFn)(TorqueCall call, uint32_t index, uint32_t *out);

// `this_object`.
typedef TorqueStatus (*TorqueThisObjectFn)(TorqueCall call, uint32_t *out);

// `return_str` and `return_error`.
typedef TorqueStatus (*TorqueReturnStrFn)(TorqueCall call, struct TorqueStr value);

// `return_int`.
typedef TorqueStatus (*TorqueReturnIntFn)(TorqueCall call, int32_t value);

// `return_float`.
typedef TorqueStatus (*TorqueReturnFloatFn)(TorqueCall call, float value);

// `return_bool`.
typedef TorqueStatus (*TorqueReturnBoolFn)(TorqueCall call, uint32_t value);

// `get_global`.
typedef TorqueStatus (*TorqueGetGlobalFn)(TorqueCall call, struct TorqueStr name, struct TorqueStr *out);

// `set_global`.
typedef TorqueStatus (*TorqueSetGlobalFn)(TorqueCall call, struct TorqueStr name, struct TorqueStr value);

// One vertex of a pass's triangles: a position (in the space the draw call names) and a linear RGBA color.
typedef struct TorqueVertex {
    // The position.
    float position[3];
    // The color, red, green, blue and alpha from 0 to 1 (alpha blends over what is there).
    float color[4];
} TorqueVertex;

// `draw_triangles`.
typedef TorqueStatus (*TorqueDrawTrianglesFn)(TorqueDraw draw, uint32_t space, const struct TorqueVertex *vertices, size_t count);

// `log`.
typedef TorqueStatus (*TorqueLogFn)(uint64_t handle, uint32_t level, struct TorqueStr message);

// The engine's function table, handed to `torque_plugin_init`. It stays valid while the extension is loaded, which is
// until the process ends: keep the pointer.
//
// `abi_version` and `size` come first in every version: read them before anything else, and give up on a version that is
// not the one the extension was built for.
//
// **Threads.** The engine may call into an extension (a console function, a pass) from any of its threads, but never twice
// at once: it makes one call into an extension at a time, across all its threads, and refuses a call that would re-enter
// the extension on the thread that is already inside it. So an extension's own state needs no lock against the engine,
// but it may not count on one thread, or on thread-local storage, staying the same from call to call.
typedef struct TorqueApi {
    // The engine's [`TORQUE_ABI_VERSION`].
    uint32_t abi_version;
    // The size of this table in bytes.
    uint32_t size;
    // This init's handle, for the `register_*` functions.
    TorqueInit init;
    // Declares a console function (only during `torque_plugin_init`).
    TorqueRegisterNativeFn register_function;
    // Declares a console method of the class `decl.ns`: its object is `this_object` (only during init).
    TorqueRegisterNativeFn register_method;
    // Declares a global variable (`name` with its `$`) set to `default_value` before scripts run (only during init).
    TorqueRegisterGlobalFn register_global;
    // Declares a render pass (only during init).
    TorqueRegisterPassFn register_render_pass;
    // How many script arguments the call has (not counting the name or a method's object).
    TorqueArgCountFn arg_count;
    // Script argument `index` (from 0) as bytes, valid until the call returns.
    TorqueArgStrFn arg_str;
    // Script argument `index` as an integer, by the script engine's rules (`dAtoi`).
    TorqueArgIntFn arg_int;
    // Script argument `index` as a float, by the script engine's rules (`dAtof`, rounded to 32 bits).
    TorqueArgFloatFn arg_float;
    // Script argument `index` as a truth value (1 or 0), by the script engine's rules (`dAtob`).
    TorqueArgBoolFn arg_bool;
    // A method call's object id.
    TorqueThisObjectFn this_object;
    // Sets the call's result to text (copied).
    TorqueReturnStrFn return_str;
    // Sets the call's result to an integer.
    TorqueReturnIntFn return_int;
    // Sets the call's result to a float.
    TorqueReturnFloatFn return_float;
    // Sets the call's result to a truth value (any value but 0 is true).
    TorqueReturnBoolFn return_bool;
    // Fails the call: the console prints `message` as an error with the function's name and its usage, and the call
    // returns the empty string.
    TorqueReturnStrFn return_error;
    // A global variable's value (`name` with its `$`), valid until the next `get_global` or the call's end; empty when
    // it is not set.
    TorqueGetGlobalFn get_global;
    // Sets a global variable (`name` with its `$`).
    TorqueSetGlobalFn set_global;
    // Draws triangles in a pass's frame: `count` vertices (a multiple of 3) in `space` (`TORQUE_SPACE_`), copied.
    TorqueDrawTrianglesFn draw_triangles;
    // A message (`TORQUE_LOG_`): to the running script engine's console during a call, to the engine's log otherwise.
    // `handle` is the live init, call or draw handle, or 0.
    TorqueLogFn log;
} TorqueApi;

// What an extension says about itself, filled in by `torque_plugin_init`.
typedef struct TorquePluginInfo {
    // The [`TORQUE_ABI_VERSION`] the extension was built against.
    uint32_t abi_version;
    // Its stable id, such as `example.native-sample`: ASCII letters, digits, `_`, `-`, `.` and `:`.
    struct TorqueStr id;
    // Its version, such as `0.1.0`.
    struct TorqueStr version;
    // Its network version when [`has_net_version`](Self::has_net_version) is 1: a server and a client must then both have
    // it, at the same version.
    uint32_t net_version;
    // 1 when it changes what goes over the network, else 0.
    uint32_t has_net_version;
} TorquePluginInfo;

// `torque_plugin_init`: the extension's entry point.
typedef TorqueStatus (*TorquePluginInitFn)(const struct TorqueApi *api, struct TorquePluginInfo *out);

// It worked.
#define TORQUE_OK 0

// The other side speaks another ABI version.
#define TORQUE_ERR_ABI 1

// A value was refused: a null pointer, a length over the limit, text that is not UTF-8, an unknown kind, stage or flag.
#define TORQUE_ERR_INVALID 2

// The handle is not live: it belongs to a call that has returned, or to another thread.
#define TORQUE_ERR_HANDLE 3

// The code that was called panicked (the extension is disabled when its own code does).
#define TORQUE_ERR_PANIC 4

// The request was understood and refused: a name already taken, an extension that is disabled, an init that failed.
#define TORQUE_ERR_FAILED 5

// An index past the end, or more than a limit allows (the arguments of a call, the vertices of a frame).
#define TORQUE_ERR_RANGE 6

#ifdef __cplusplus
extern "C" {
#endif

/* The two symbols an extension exports: the ABI version it was built against, and its entry point. */
TORQUE_EXPORT extern const uint32_t torque_plugin_abi_version;
TORQUE_EXPORT TorqueStatus torque_plugin_init(const TorqueApi *api, TorquePluginInfo *out);

#ifdef __cplusplus
}
#endif

#endif /* TORQUE_PLUGIN_H */
