#!/usr/bin/env python3
"""Ensure every Steam profile has a multi-tenant Discord shortcut.

For each profile's shortcuts.vdf:
- entries that launch Discord directly (flatpak run com.discordapp.Discord,
  /usr/bin/discord, …) are rewritten to exec ~/.local/bin/game-mode-discord.
  Stored appids are preserved (Steam does not re-key on edit), so artwork keeps
  working.
- profiles with NO Discord entry get one created — without a tile a new user
  has no way to reach Discord's first-time login from game mode, and couchcord
  waits on a Discord that can never start.

Re-run after adding a Steam profile (or just re-run install.sh; idempotent).
Steam must be closed (it rewrites shortcuts.vdf on exit) unless --dry-run.
A .bak copy is left next to every modified file.
"""
import glob
import os
import shutil
import struct
import subprocess
import sys
import zlib

WRAPPER = os.path.expanduser("~/.local/bin/game-mode-discord")
USERDATA = os.environ.get(
    "COUCHCORD_STEAM_USERDATA", os.path.expanduser("~/.steam/steam/userdata")
)
DRY = "--dry-run" in sys.argv


def parse(data):
    pos = [0]

    def byte():
        pos[0] += 1
        return data[pos[0] - 1]

    def cstr():
        end = data.index(b"\x00", pos[0])
        s = data[pos[0]:end].decode(errors="replace")
        pos[0] = end + 1
        return s

    def obj():
        out = {}
        while True:
            t = byte()
            if t == 8:
                return out
            k = cstr()
            if t == 0:
                out[k] = obj()
            elif t == 1:
                out[k] = cstr()
            elif t == 2:
                out[k] = struct.unpack("<I", data[pos[0]:pos[0] + 4])[0]
                pos[0] += 4
    byte()
    cstr()
    return obj()


def serialize(shortcuts):
    def s(k, v):
        return b"\x01" + k.encode() + b"\x00" + v.encode() + b"\x00"

    def i(k, v):
        return b"\x02" + k.encode() + b"\x00" + struct.pack("<I", v)

    def obj(d):
        out = b""
        for k, v in d.items():
            if isinstance(v, dict):
                out += b"\x00" + k.encode() + b"\x00" + obj(v) + b"\x08"
            elif isinstance(v, int):
                out += i(k, v)
            else:
                out += s(k, v)
        return out

    body = b""
    for k, v in shortcuts.items():
        body += b"\x00" + k.encode() + b"\x00" + obj(v) + b"\x08"
    return b"\x00shortcuts\x00" + body + b"\x08\x08"


def is_discord_launch(entry):
    exe = entry.get("Exe", "")
    opts = entry.get("LaunchOptions", "")
    if WRAPPER in exe:
        return False  # already rewired
    hay = (exe + " " + opts).lower()
    return "com.discordapp.discord" in hay or "/discord" in hay.replace("\\", "/")


def new_entry():
    appid = zlib.crc32(f'"{WRAPPER}"Discord'.encode()) | 0x80000000
    return {
        "appid": appid,
        "appname": "Discord",
        "Exe": f'"{WRAPPER}"',
        "StartDir": f'"{os.path.expanduser("~")}"',
        "icon": "",
        "ShortcutPath": "",
        "LaunchOptions": "",
        "IsHidden": 0,
        "AllowDesktopConfig": 1,
        "AllowOverlay": 1,
        "OpenVR": 0,
        "Devkit": 0,
        "DevkitGameID": "",
        "DevkitOverrideAppID": 0,
        "LastPlayTime": 0,
        "tags": {"0": "Games"},
    }


def main():
    real_userdata = "COUCHCORD_STEAM_USERDATA" not in os.environ
    if (
        not DRY
        and real_userdata
        and subprocess.run(["pgrep", "-x", "steam"], capture_output=True).returncode == 0
    ):
        print("error: close Steam first (it overwrites shortcuts.vdf on exit), or use --dry-run")
        return 1
    for userdir in sorted(glob.glob(os.path.join(USERDATA, "*/"))):
        profile = os.path.basename(userdir.rstrip("/"))
        if not profile.isdigit():
            continue
        path = os.path.join(userdir, "config", "shortcuts.vdf")
        exists = os.path.exists(path)
        shortcuts = parse(open(path, "rb").read()) if exists else {}
        rewired = []
        has_wrapper = False
        for key, entry in shortcuts.items():
            if not isinstance(entry, dict):
                continue
            if WRAPPER in entry.get("Exe", ""):
                has_wrapper = True
            elif is_discord_launch(entry):
                entry["Exe"] = f'"{WRAPPER}"'
                entry["LaunchOptions"] = ""
                entry["StartDir"] = f'"{os.path.expanduser("~")}"'
                rewired.append(entry.get("appname", key))
                has_wrapper = True
        created = False
        if not has_wrapper:
            free = next(str(n) for n in range(len(shortcuts) + 1) if str(n) not in shortcuts)
            shortcuts[free] = new_entry()
            created = True
        if not (rewired or created):
            print(f"profile {profile}: ok (multi-tenant Discord shortcut present)")
            continue
        action = " + ".join(
            filter(None, [f"rewire {rewired}" if rewired else "", "create Discord shortcut" if created else ""])
        )
        if DRY:
            print(f"[dry-run] profile {profile}: would {action}")
            continue
        if exists:
            shutil.copy(path, path + ".bak")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        open(path, "wb").write(serialize(shortcuts))
        print(f"profile {profile}: {action}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
