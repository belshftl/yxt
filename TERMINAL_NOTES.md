-----

## kitty implementations

| Name      | As of commit           | Progressive enhancements          | Max stack depth                                         | Spec compliant? (see notes below if no)                                                                                                     | Affects us?                                                                |
| --------- | ---------------------- | --------------------------------- | ------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| kitty     | `9710eb446` @ `master` | All                               | 8 (`kitty/screen.h:167`)                                | **Yes**                                                                                                                                     | **No**                                                                     |
| foot      | `2705e36f` @ `master`  | All                               | 8 (`terminal.h:245`)                                    | **Yes**                                                                                                                                     | **No**                                                                     |
| ghostty   | `44f2a44df` @ `main`   | All                               | 8 (`src/terminal/kitty/key.zig:9`)                      | **Yes**                                                                                                                                     | **No**                                                                     |
| wezterm   | `2b56c4688` @ `main`   | All                               | 128 (`term/src/terminalstate/performer.rs:553`)         | **Yes**, but the protocol is off by default; see below                                                                                      | **No**; unanswered `CSI ? u` is exactly how we detect kitty as unsupported |
| rio       | `7ae087500b` @ `main`  | 1/2/8/16; 4 omits base layout key | 8 (`rio-vt/src/crosswords/mod.rs:424`)                  | **Yes**                                                                                                                                     | **No**; we never request flag 4                                            |
| alacritty | `d692748d` @ `master`  | 1/2/8/16; 4 omits base layout key | 4096, sort of (`alacritty_terminal/src/term/mod.rs:48`) | No, but **almost yes**; violates stack overflow behavior, maintainer claims won't fix                                                       | **No**; see "stack behavior violations and us"                             |
| contour   | `0ad6bdbe` @ `master`  | 1/2/8; 4 omits base layout key    | 32 (`src/vtbackend/input/InputGenerator.hpp:538`)       | **No**; falsely advertises flag 16; no separate altscreen stack; violates stack overflow behavior; wrong push param default; see more below | **Yes**; see "contour and us"                                              |
| iTerm2    | `28dec2146` @ `master` | All                               | 1024, sort of (`sources/VT100/VT100Terminal.m:3187`)    | **No**; violates stack overflow behavior; `CSI = flags ; mode u` is stickier than it should be and leaks across screens                     | **Yes**; see "iTerm2 and us"                                               |

### base layout key omission

The form is `CSI key : shifted : base ; mods u`, and the aforementioned "4 omits base layout key" terminals omit the `: base` form; Ctrl+C on a Russian layout is `CSI 1089::99;5u` on kitty/foot/ghostty/wezterm/iTerm2 but `CSI 1089;5u` on rio/alacritty/contour. The spec's wording is permissive and says terminals *can* send two additional codepoints:
> If alternate key reporting is requested by the program running in the terminal, the terminal can send two additional Unicode codepoints, the shifted key and base layout key, separated by colons.

So this is not a spec violation in of itself.

### wezterm

implements every enhancement, but ships with the protocol disabled and gated on `enable_kitty_keyboard = true`. Under the default config, `CSI ? u` is unanswered, so applications correctly detect the protocol as unsupported.

### alacritty

has a bug where, once the keyboard mode stack is full, attempting to push again evicts from the *window title stack* instead:
```rs
// alacritty_terminal/src/term/mod.rs:1295
if self.keyboard_mode_stack.len() >= KEYBOARD_MODE_STACK_MAX_DEPTH {
    let removed = self.title_stack.remove(0);
    // ...
}
self.keyboard_mode_stack.push(mode);
```

The keyboard stack keeps growing unbounded past 4096, and `title_stack.remove(0)` panics if the title stack is empty. As such, going above max stack depth panics rather than evicting from the bottom, unless a title has been pushed first. The panic happens on the `PTY reader` thread (`alacritty_terminal/src/event_loop.rs:206`), whose `JoinHandle` is dropped without ever being joined (`alacritty/src/window_context.rs:227`); `FairMutex` wraps `parking_lot::Mutex`, which does not poison and releases the mutex instead; no crate sets `panic = "abort"`, and the panic hook is Windows-only. Therefore, the process survives with a dead PTY reader, with the window still up but functionally dead as it stops processing terminal I/O. The issue happens to be masked by the fact the stack depth is really large.

This issue has been pointed out, with a fix proposed, in [#8957](https://github.com/alacritty/alacritty/issues/8957), and dismissed by a maintainer saying "There's no point in trying to defend against a DOS from a malicious application.", so this should be treated as an intentional spec violation and not merely a to-be-fixed bug.

### contour

is the least compliant of the three violators. All line numbers in the format `:123` with no mentioned file are in `src/vtbackend/input/InputGenerator.hpp`, and `cpp:123` is in `src/vtbackend/input/InputGenerator.cpp`. These are roughly sorted by highest -> lowest severity, but the order is still mostly arbitrary.

Progressive enhancement flag 16 is practically unimplemented; the only producer is `numpadAssociatedText` @ `cpp:556`, consumed later in the same file (`cpp:611-612`); `generateChar` encodes no text field at all (`cpp:371`). On its own, this would not be problematic, but it also advertises support for flag 16:
```cpp
// src/vtbackend/screen/Screen.cpp:7164
case CSIUENTER: {
    auto const flags = KeyboardEventFlags::fromValue(seq.paramOr(0, 1));
    _terminal->keyboardProtocol().enter(flags);
    return ApplyResult::Ok;
}
case CSIUQUERY: {
    reply("\033[?{}u", _terminal->keyboardProtocol().flags().value());
    return ApplyResult::Ok;
}
```
`enter(flags)` pushes the flags verbatim (see definition below), and `CSIUQUERY` echoes them back, also verbatim. This means that applications relying on flags 8|16 receive associated text for numpad keys and nothing else.

By extension of the above, `CSI > 200 u` + `CSI ? u` answers `CSI ? 200 u`.

There is one stack per `Terminal` (`:833`) and `Terminal::setScreen` (`src/vtbackend/screen/Terminal.cpp:4585`) does not swap it.

`CSI > flags u` has the wrong default; see `case CSIUENTER:` above. `paramOr` is `T paramOr(size_t parameterIndex, T defaultValue)`, so `seq.paramOr(0, 1)` defaults omitted `flags` to 1, when the spec says 0. As such, `CSI > u` enables disambiguation, when it should push a clean-slate mode.

A push at max stack depth is dropped silently:
```cpp
// :540
constexpr void enter(KeyboardEventFlags flags) noexcept
{
    if (stackDepth() < MaxStackDepth)
        _flags.at(++_currentStackTop) = flags;
}
```
Which, in practice, means that the corresponding `CSI < n u` pops a stack entry belonging to some outer program.

A pop that empties the stack doesn't clear `CSI = flags ; mode u`'s flags. The bottom stack slot holds the live flag set, and is the only slot `CSI = flags ; mode u` ever writes (`flags()` @ `:550`), but `leave` only moves the index and never clears what it leaves behind:
```cpp
// :572
constexpr void leave(size_t n = 1) noexcept { _currentStackTop -= std::min(n, _currentStackTop); }
```
```cpp
// src/vtbackend/screen/Screen.cpp:7186
case CSIULEAVE: {
    auto const count = seq.paramOr<size_t>(0, 1);
    _terminal->keyboardProtocol().leave(count);
    return ApplyResult::Ok;
}
```
kitty clears the popped entry instead (`kitty/screen.c:2121`), so `CSI = 5 u` + `CSI < 1 u` resets the mode to 0 in kitty and leaves it at 5 in contour.

Numpad keys with numlock on are reported as escape codes under plain disambiguation. `src/contour/session/SessionInput.cpp:511-532` maps the numpad digits to `Key::Numpad_0`..`Key::Numpad_9` and infers `LockKey::NumLock` from the keycode, and `generateKey` then folds that lock into the modifier value, so numpad 1 is `CSI 57400;129u`. kitty (`kitty/key_encoding.c:457-467`) and foot (`input.c:1329`) send the text `1` instead: a keypad key that produces text is not one of the "non text keypad keys" that disambiguation is meant to report separately.

Numpad nav keycodes (57416-57427) are unimplemented and marked with a `// TODO:` comment (`cpp:537-549`), so under flag 1 the numpad arrows / home/end / pageup/pagedown / insert / delete are indistinguishable from the non-numpad ones.

`isModifierKey` (`:298`) omits scrolllock, so scrolllock is reported as `CSI 57359u` without flag 8. kitty withholds modifier keys unless flag 8 is set, and counts scrolllock among them (`kitty/keys.c:38`).

### iTerm2

has a bug where, once the keyboard mode stack is full, attempting to push again clears *the entire stack*:
```objective-c
// sources/VT100/VT100Terminal.m:3187
const NSInteger maxCount = 1024;
if (array.count < maxCount) {
    return;
}
[array removeObjectsInRange:NSMakeRange(array.count - maxCount, maxCount)];
```
At `array.count == 1024` the range evaluates to `(0, 1024)`, so the entire stack gets discarded. As such, 1023 is the functional max usable depth. Like alacritty, the issue happens to be masked by the fact the stack depth is really large.

Additionally, `CSI = flags ; mode u` sets a base value outside of the stack named `_keyReportingFlags` (see lines 2520-2548 `case VT100CSI_SET_KEY_REPORTING_MODE` for where it gets set, and lines 663-668 `(VT100TerminalKeyReportingFlags)keyReportingFlags` for where it gets used), which doesn't get cleared on a `CSI < n u` that empties the stack (see lines 3203-3215 `(void)popKeyReportingModes:(int)count`). In other words, `CSI = 5 u` followed up by `CSI < 1 u` leaves the mode at 5 when kitty resets it.

That base value is also shared between the screens, while the stacks are not: `currentKeyReportingModeStack` (lines 670-677) picks `_mainKeyReportingModeStack` or `_alternateKeyReportingModeStack` per screen, but both fall back to the one `_keyReportingFlags` ivar (line 229).

Lastly, `CSI > Pm m` (XTMODKEYS) wipes key reporting state: `case VT100CSI_SET_MODIFIERS` (lines 2967-2997) zeroes `_keyReportingFlags` and empties the current stack, and `case VT100CSI_RESET_MODIFIERS` (lines 2952-2965) empties the stack. This is intentional:
```objective-c
// sources/VT100/VT100Terminal.m:2988

// The protocol described here:
// https://sw.kovidgoyal.net/kitty/keyboard-protocol/#progressive-enhancement
// is flawed because if CSI > 4 ; 0 m pops the stack it would leave
// it in the wrong state if CSI > 4 ; 1 m were sent twice.
// CSI m will nuke the stack and CSI u will respect it.
self.dirty = YES;
[self.currentKeyReportingModeStack removeAllObjects];
```

But does practically mean `CSI > 4 ; 2 m` wipes out whatever an outer program pushed.

### stack behavior violations and us

Alacritty and iTerm2's stack-overflow bugs aren't practically reachable beacuse we push exactly once per session: `TermProxy::enable_sequence` sends one `CSI > flags u` at startup and `RestoreGuard` sends one `CSI < u` at exit; `term::negotiate::sync_terminal_flags` sends `CSI = flags u`, not a push.

A child spamming pushes doesn't reach them either; `TermProxy::handle_event` classifies the child's kitty controls as `Suppress` and, instead of forwarding them, sends them to our own stack in `term::mode` instead, which is a depth of 8 and has correct overflow behavior.

"A pop that empties the stack doesn't clear `CSI = flags ; mode u`" is problematic; if we send any such sequence during the session, which is as soon as the child changes its own flags, the exit pop leaves the terminal holding our flag set rather than restoring it. The shell or whatever other outer program there is then gets kitty key reports it never asked for. A potential fix would be to not push/pop entirely; we already query the current flags at startup, so we could set our new flags and then set the original fetched value back on exit, not using the stack at all.

### contour and us

Aside from the stack behavior violations shared with some other terminals, problematic is also:

- **Flag 16;** we request `8|16` (`ALL_KEYS_KITTY_FLAGS`) if a mapping needs a key that legacy can't report. If associated text doesn't work, a shifted char is received as its base codepoint + a shift modifier, rather than as the shifted char, so `key('2'~, shift)` never fires and `key('2'~)` incorrectly matches instead. Any terminal that doesn't implement flag 16 would behave this way; contour is just the one that falsely advertises it, so rather than failing at startup claiming the terminal doesn't support the kitty protocol, it silently misbehaves.
- **Numpad w/ numlock latched** is reported as a functional keycode rather than the text, so it gets decoded by `kitty::key_from_kitty_codepoint` as a keypad **nav** key and the mapping for the digit never fires.
- **Numpad nav keycodes are unimplemented**, so `key(kp_left)` and friends never fire.
- **A max depth push is discarded silently**, so our exit pop would remove an outer program's entry instead of ours if the stack is full. This is unlikely in practice, as it'd need 32 nested pushers.

We're **not** affected by:

- **No separate altscreen stack;** the per-screen tracking in `term::mode` models the child, not the terminal, and we always send set commands (`CSI = flags ; mode u`) rather than stack pushes.
- **Wrong `CSI > u` parameter default;** `kitty::push_flags_sequence` sends explicit flags, not the short form.

This is a significant breakage, and there isn't much we can do here; we're targeting the *kitty keyboard protocol*, so a buggy implementation of said protocol is expected to misbehave. An advisory detection guard that makes it fail at startup can be added later.

### iTerm2 and us

The XTMODKEYS wipe behavior is not an issue right now, since we only ever decode `modifyOtherKeys` reports and never send XTMODKEYS. It does, however, mean a future `modifyOtherKeys` protocol rank being usable alongside kitty on iTerm2 is not a possibility: negotiating it entails sending `CSI > 4 ; 2 m`, which destroys the kitty keyboard state, including whatever an outer program pushed before us. The two protocols are mutually exclusive on iTerm2, and getting it wrong not only misbehaves but breaks outer programs.

The `CSI = flags ; mode u` stickiness is, though; see above in "stack behavior violations and us".

-----

## xterm `modifyOtherKeys`

Empirically derived on a fresh xterm install on Arch; `xrdb -query` came up empty, so it's either defaults or popular-distro-defaults:
- **Shifted characters are escaped once mode 2 has been set;** after explicitly sending `CSI > 4 ; 2 m`, shift+a is `CSI 27;2;65~`, i.e. the *shifted* glyph as `code` + the shift bit set in `mods`. See the open question below before reading anything into this.
- **Reports have to be decoded whether or not we asked for the mode;** nothing else uses parameter 27, so decoding it costs nothing and cannot be confused for anything, and a program that sets mode 2 and is killed before restoring leaves the terminal in it regardless of what we do.
- **Mode 1 is not enough to disambiguate ctrl+h;** under `CSI > 4 ; 1 m` it's still BS/0x08, only mode 2 reports it as `CSI 27;5;104~`.
- **`CSI > 4 m` with no parameter reports 0 afterwards;** sending `CSI > 4 ; 2 m` then `CSI > 4 m` leaves `CSI ? 4 m` answering 0. Can't tell whether that means "sets 0" or "restores whatever it was" until the initial-value question below is figured out.
- **Keys that don't change:** function and cursor keys keep their ordinary forms and mods bitfield (`ESC OP` for f1, `CSI 1;5P` for ctrl+f1, `CSI A` for up, `CSI 1;5A` for ctrl+up), and backspace stays `0x7f` even with ctrl held, since it is not an "other key". Keys whose base character is a control code appear under that code: ctrl+enter is `CSI 27;5;13~` and ctrl+tab is `CSI 27;5;9~`.

Things that still need more testing:
- **What the initial value actually is, and whether mode 2 really escapes shift on its own.** Sending `CSI ? 4 m` answered `CSI > 4 ; 2 m` before anything had been sent; the logical conclusion is that mode 2 is the default, which would mean stock xterm breaks typing capital letters, which is obviously wrong, so at least one of these is being misread.
- **Whether meta is encoded.** Sending Left Meta + A through UTM didn't emit `CSI 27 ; ... ~` and just sent `a` as plain text, but it seems to just map to mod4/super (verified ad hoc by accidentally triggering i3's "focus parent container" default binding with `$mod` set to mod4) so the meta bit wasn't actually ever tested.

VTE already never claims to implement `modifyOtherKeys` anywhere, so this info isn't new, but testing it in xfce4-terminal confirms it: sending `CSI > 4 ; 2 m` doesn't change any behavior, and `CSI ? 4 m` is unanswered.

### DA1 as a query sentinel

Every terminal tested (caveat: that's two terminals) answers `CSI c` even if it ignored the capability query that was sent alongside, making "doesn't support this" distinguishable from "hasn't answered yet", and is why `term::query` adds DA1 at the end of the batch.

Replies seen, which also serve to identify the two: xterm `CSI ?64;1;2;6;9;15;17;18;21;22;28 c`, xfce4-terminal `CSI ?61;1;21;22;28 c`.
