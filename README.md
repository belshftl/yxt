# yxt
is a decent key injector/remapper/grabber for terminal programs

---

## a quick note

this is very unfinished right now, it's probably gonna feel a decent bit unpolished/rough. the big thing holding this back from being decent is a lack of a "negotiate a terminal protocol that the child doesn't know about and translate it for the child" feature to make modern capabilities only depend on modern terminals and not the child happening to explicitly opt into the protocol, i'm planning to add that as `@protocol` but that's not a thing yet

## getting started

make a config file. you can either pass one on the command line:
```sh
yxt -c my-awesome-config.conf -- someprogram
```

or put it into `~/.config/yxt/implicit/PROGRAMNAME.conf`, with `PROGRAMNAME` being the binary's filename (without the file extension, if there is one), so `yxt someprogram` and `yxt /some/path/someprogram.elf` will both search for a file named `~/.config/yxt/implicit/someprogram.conf`.

every config file (that isn't pulled from an `@include` in another one) must have a `@version` directive in it specifying the config format's version as the first statement in the file. the current version is `1`, so you have to put:
```
@version 1
```
at the top of every root (not `@include`) config file. non-root ones can have it too, it's harmless there.

the four statement types are:
- directives: start with `@`, so `@version`, `@include`, etc.
- definitions: start with `define `, so `define group "mygroup"`, etc.
- mappings: this is mostly what you actually care about, in the format `x => y` or `y <= x`, the next section is about these
- option assignments: control config options, simple variable-assignment style syntax like `log_file = "/some/path/yxt.log"`

POSIX-shell-style comments are supported (comments use `#`, block/inline comments aren't a thing). blank lines are ignored, as is leading/trailing whitespace.

## "so how do i change keybinds"

the entire point of the program is to map sources to targets. everything else is just sugar on top. a source is something like keyboard input or receiving a signal, and a target is something like sending keyboard input or executing a shell command. there's more, but these are the basic examples.

a key is either a named key (esc, enter, arrow keys, etc.) or a pair of unicode characters, one for the character the key produces without shift held down (e.g `a`) and one for with shift (`A`). it's a bit inconvenient but the only way to portably detect shift modifiers, sorry

here's a simple, contrived example, remapping `x` to enter and nothing else:
```
@version 1
key('x'~'X') => inherit_key(enter) # or send_key(enter)
```
`'x'~'X'` defines a pair with the left side being unshifted and the right being shifted. `inherit_key` inherits whether the key is a press/repeat/release (more on this later) and its modifiers, and requires the source to provide key info, while `send_key` just sends a keypress with the specified modifiers. note that unlike the rest, `send_key` takes either a named key or a single character, not a pair.

here's a more practical example (that's also closer to how you'd write a config), adding hjkl navigation to some program with arrow key nav:
```
@version 1
key('h'~, any) => inherit_key(left)
key('j'~, any) => inherit_key(down)
key('k'~, any) => inherit_key(up)
key('l'~, any) => inherit_key(right)
```

omitting a side of a pair infers the other side; this is only supported for keys on standard US layouts (e.g QWERTY). the second argument specifies the modifiers to match, defaulting to `none`; `any` means any modifiers, so shift+k (`K`) maps to shift+up, ctrl+k maps to ctrl+up, etc.

omitting the left side of a pair works as you'd expect; `key(~'!')` is equivalent to `key('1'~'!', shift)`.

you can also do `y <= x` as syntactic sugar for `x => y`; this will come up in a bit too.

## beyond keys

say you have some program with a reload bind of ctrl+r, and you want to have it automatically reload when, say, a file changes. you can write a small script that watches the file with inotify or something and then do:

```
@version 1
@service "reload-watcher" exec("my-inotify-watcher", "/path/to/somefile") # or sh("...") for a shell command
sockdata_utf8("reload") => send_key('r', ctrl)
```

yxt always creates a datagram socket, and children it spawns get the environment variable `YXT_SOCK` set. here, when it gets the datagram `reload` (as UTF-8), it sends `r` to the child. same example but with `SIGUSR1` (the child gets the PID as `YXT_PID`):
```
@version 1
@service "reload-watcher" exec("my-inotify-watcher", "/path/to/somefile")
signal("SIGUSR1") => send_key('r', ctrl)
```

you can execute commands as targets; here's another practical example, adding a rescan bind to `nmtui`:
```
@version 1
key('r'~) => exec("nmcli", "device", "wifi", "rescan")
```
this could also be `sh("nmcli device wifi rescan")` if you value config readability over not spawning a shell.

finally, you can define and fire your own "source"/"target": groups. example:
```
@version 1
define group "reload"

key(space) => group("reload")
signal("SIGUSR1") => group("reload")
sockdata_utf8("reload") => group("reload")
group("reload") => send_key('r', ctrl)
```
this sends `r` on space, SIGUSR1, and `reload` in UTF-8 on the socket. this is nicer with the `<=` sugar:
```
@version 1
define group "reload"

group("reload") <= key(space)
group("reload") <= signal("SIGUSR1")
group("reload") <= sockdata_utf8("reload")
group("reload") => send_key('r', ctrl)
```

groups are special in that they're both sources and targets, and they're full-fledged sources/targets too; a self-map like `group("x") => group("x")` is caught during config parsing, but other kinds of cycles like `a -> b -> a` are runtime errors.

full list of sources:
- `signal`
- `sockdata_utf8`
- `key`
- `group`

full list of targets:
- `send_key`
- `inherit_key`
- `group`
- `exec`
- `sh`

[TODO finish this]
