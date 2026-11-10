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

// How deep calls back into one extension may nest on one thread ([`TorqueApi::call_function`] running a script that
// calls the extension again); the call past it fails with [`TORQUE_ERR_LIMIT`].
#define TORQUE_MAX_NESTING 64

// The steps an outermost call into an extension may [`charge`](TorqueApi::charge) when its script engine has no step
// limit of its own.
#define TORQUE_DEFAULT_STEPS 50000000

// The longest script file [`TorqueApi::read_script`] reads (bytes).
#define TORQUE_MAX_SCRIPT ((16 * 1024) * 1024)

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

// The native opens, writes, deletes or exports files. The engine's default call policy keeps script languages from
// calling a native with this flag or [`TORQUE_NATIVE_CODE`], [`TORQUE_NATIVE_ENGINE`] or [`TORQUE_NATIVE_PERMISSIONS`],
// so a native with one of these effects must declare it.
#define TORQUE_NATIVE_FILES 4

// The native runs code from text, or a function or method it names.
#define TORQUE_NATIVE_CODE 8

// The native changes what the engine runs as a whole: switches or loads mods, deletes every datablock or variable,
// quits.
#define TORQUE_NATIVE_ENGINE 16

// The native connects to, or sends to, an address its caller names.
#define TORQUE_NATIVE_NETWORK 32

// The native grants, revokes or defines permissions or roles.
#define TORQUE_NATIVE_PERMISSIONS 64

// The native writes a variable whose name one of its arguments gives: the call policy checks that write as a write of
// the variable, taking any argument for the name.
#define TORQUE_NATIVE_VARIABLES 128

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
    // [`TORQUE_NATIVE_DEV`], [`TORQUE_NATIVE_EXACT_USAGE`] and its effects ([`TORQUE_NATIVE_FILES`],
    // [`TORQUE_NATIVE_CODE`], [`TORQUE_NATIVE_ENGINE`], [`TORQUE_NATIVE_NETWORK`], [`TORQUE_NATIVE_PERMISSIONS`],
    // [`TORQUE_NATIVE_VARIABLES`]), or 0.
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

// `call_function`.
typedef TorqueStatus (*TorqueCallFunctionFn)(TorqueCall call, struct TorqueStr ns, struct TorqueStr name, const struct TorqueStr *args, size_t count, struct TorqueStr *out);

// `call_method`.
typedef TorqueStatus (*TorqueCallMethodFn)(TorqueCall call, uint32_t object, struct TorqueStr name, const struct TorqueStr *args, size_t count, struct TorqueStr *out);

// `has_method`.
typedef TorqueStatus (*TorqueHasMethodFn)(TorqueCall call, uint32_t object, struct TorqueStr name, uint32_t *out);

// `find_object`.
typedef TorqueStatus (*TorqueFindObjectFn)(TorqueCall call, struct TorqueStr name, uint32_t *out);

// `get_field`.
typedef TorqueStatus (*TorqueGetFieldFn)(TorqueCall call, uint32_t object, struct TorqueStr name, struct TorqueStr index, struct TorqueStr *out);

// `set_field`.
typedef TorqueStatus (*TorqueSetFieldFn)(TorqueCall call, uint32_t object, struct TorqueStr name, struct TorqueStr index, struct TorqueStr value);

// A console function a script language defines at run time, in the call's script engine only (`define_function`).
// Calls of it go to the language's `call_defined` with `id`.
typedef struct TorqueDefineDecl {
    // The namespace (empty for the global one) or class: `Player` makes `Player::name`, which a method call on a player
    // finds, with the object first among the arguments.
    struct TorqueStr ns;
    // The name scripts call.
    struct TorqueStr name;
    // The package it goes into, so that deactivating the package hides it again: empty for none, else one
    // [`TorqueApi::script_package`] gave the language in this script engine.
    struct TorqueStr package;
    // Its usage text (empty for none).
    struct TorqueStr usage;
    // Handed back to `call_defined`.
    uint64_t id;
} TorqueDefineDecl;

// `define_function`.
typedef TorqueStatus (*TorqueDefineFunctionFn)(TorqueCall call, const struct TorqueDefineDecl *decl);

// `charge`.
typedef TorqueStatus (*TorqueChargeFn)(TorqueCall call, uint64_t steps);

// A script engine (one server's or one client's): the same number for its whole life, never reused for another.
typedef uint64_t TorqueVm;

// What [`TorqueApi::vm_info`] tells about the script engine of a call.
typedef struct TorqueVmInfo {
    // The script engine.
    TorqueVm vm;
    // [`TORQUE_SIDE_SERVER`] or [`TORQUE_SIDE_CLIENT`]; [`TORQUE_SIDE_BOTH`] for one the engine made outside a server or a
    // client (a tool's).
    uint32_t side;
    // Its step limit for one outermost run, or 0 when it has none ([`TORQUE_DEFAULT_STEPS`] then bounds an outermost
    // call into the extension).
    uint64_t step_limit;
} TorqueVmInfo;

// `vm_info`.
typedef TorqueStatus (*TorqueVmInfoFn)(TorqueCall call, struct TorqueVmInfo *out);

// `script_package`.
typedef TorqueStatus (*TorqueScriptPackageFn)(TorqueCall call, struct TorqueStr *out);

// `read_script`.
typedef TorqueStatus (*TorqueReadScriptFn)(TorqueCall call, struct TorqueStr path, struct TorqueStr *out);

// A script language the extension runs (`register_script_language`).
typedef struct TorqueLanguageDecl {
    // [`TORQUE_SIDE_BOTH`], [`TORQUE_SIDE_SERVER`] or [`TORQUE_SIDE_CLIENT`]: which script engines run its files.
    uint32_t side;
    // The file extension it runs, without the dot (`lua`): ASCII letters and digits, not one of TorqueScript's (`cs`,
    // `dso`, `gui`, `mis`).
    struct TorqueStr extension;
    // Runs a file (a [`TorqueRunFileFn`]).
    TorqueStatus (*run_file)(void *user, TorqueCall call, struct TorqueStr path, struct TorqueStr source);
    // Calls a function it defined (a [`TorqueCallDefinedFn`]; may be null when it defines none).
    TorqueStatus (*call_defined)(void *user, TorqueCall call, uint64_t id);
    // Told when a script engine goes (a [`TorqueVmDroppedFn`]; may be null).
    void (*vm_dropped)(void *user, TorqueVm vm);
    // Handed to the functions as it is; never read by the engine.
    void *user;
} TorqueLanguageDecl;

// `register_script_language`.
typedef TorqueStatus (*TorqueRegisterLanguageFn)(TorqueInit init, const struct TorqueLanguageDecl *decl);

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
    // Runs the console function `ns::name` (`ns` empty for a global one) with `count` arguments (at most
    // [`TORQUE_MAX_ARGS`]) and writes its result to `out`, valid until the next function of this table the extension
    // calls with this call handle, or the call's end. A function that does not exist gives the empty string (a global one
    // warns, as a script's call does). The run shares the call's budgets: [`TORQUE_ERR_LIMIT`] when it stopped at one.
    TorqueCallFunctionFn call_function;
    // Runs the method `name` of `object` (its `%this` first, then the arguments), as
    // [`call_function`](Self::call_function) does; the empty string when the object or the method does not exist.
    TorqueCallMethodFn call_method;
    // Whether `object` has a method `name` (its class's or its namespace's, script or native): 1 or 0 to `out`.
    TorqueHasMethodFn has_method;
    // The object `name` names (a name, an id, or a `/` path, as a script's `nameToID`) to `out`; [`TORQUE_ERR_FAILED`]
    // when there is none.
    TorqueFindObjectFn find_object;
    // The field `name[index]` of `object` (`index` empty for a plain field), as a script reads it, to `out` (valid as
    // [`call_function`](Self::call_function)'s result); [`TORQUE_ERR_FAILED`] when the object does not exist.
    TorqueGetFieldFn get_field;
    // Sets the field `name[index]` of `object`, as a script's assignment does; [`TORQUE_ERR_FAILED`] when the object
    // does not exist.
    TorqueSetFieldFn set_field;
    // Defines a console function in the call's script engine whose calls go to the call's script language
    // ([`TorqueLanguageDecl::call_defined`]): only during a call of a language's `run_file` or `call_defined`. Defining
    // a name again replaces it.
    TorqueDefineFunctionFn define_function;
    // Charges `steps` of work to the run's step budget: [`TORQUE_OK`] while it can pay, [`TORQUE_ERR_LIMIT`] once it is
    // spent (stop, and hand it back). A script language calls it as its code runs (at its interrupts, say).
    TorqueChargeFn charge;
    // What the call's script engine is.
    TorqueVmInfoFn vm_info;
    // During a language's `run_file`: the package the file's definitions go into (a mod's, so that switching the mod
    // off hides them), or the empty string. Valid as [`call_function`](Self::call_function)'s result.
    TorqueScriptPackageFn script_package;
    // Reads the script file `path` (a relative path in the game's files, with no `..`, ending in the extension of one of
    // the extension's languages) after the checks `exec` makes: a file a server sent is refused ([`TORQUE_ERR_FAILED`]),
    // and so is a missing one. Valid as [`call_function`](Self::call_function)'s result, at most [`TORQUE_MAX_SCRIPT`]
    // bytes.
    TorqueReadScriptFn read_script;
    // Declares a script language (only during init).
    TorqueRegisterLanguageFn register_script_language;
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

// The run stopped at a limit: the script engine's step budget or call depth, or [`TORQUE_MAX_NESTING`]. Hand it back from
// the extension's own call, so the script that called it stops too.
#define TORQUE_ERR_LIMIT 7

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
