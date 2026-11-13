#!/usr/bin/python3
"""Creates an unmapped window on the host X display and keeps it alive.

Prints the window id (decimal) and waits for SIGTERM or SIGINT. Xephyr
started with `-parent <id>` draws into this window, which is never mapped,
so nothing appears on the screen of the host display and the host receives
no input. The nested server keeps its own framebuffer, so
screenshots of the nested display still work.

Usage: hidden-parent.py WIDTH HEIGHT   (uses $HOST_DISPLAY or $DISPLAY)
"""

import ctypes
import os
import signal
import sys

width, height = int(sys.argv[1]), int(sys.argv[2])
x11 = ctypes.CDLL("libX11.so.6")
x11.XOpenDisplay.restype = ctypes.c_void_p
x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
x11.XDefaultRootWindow.restype = ctypes.c_ulong
x11.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
x11.XCreateSimpleWindow.restype = ctypes.c_ulong
x11.XCreateSimpleWindow.argtypes = [
    ctypes.c_void_p,
    ctypes.c_ulong,
    ctypes.c_int,
    ctypes.c_int,
    ctypes.c_uint,
    ctypes.c_uint,
    ctypes.c_uint,
    ctypes.c_ulong,
    ctypes.c_ulong,
]
x11.XFlush.argtypes = [ctypes.c_void_p]
x11.XCloseDisplay.argtypes = [ctypes.c_void_p]

name = os.environ.get("HOST_DISPLAY") or os.environ.get("DISPLAY")
display = x11.XOpenDisplay(name.encode() if name else None)
if not display:
    sys.exit(f"cannot open display {name}")
root = x11.XDefaultRootWindow(display)
window = x11.XCreateSimpleWindow(display, root, 0, 0, width, height, 0, 0, 0)
x11.XFlush(display)
print(window, flush=True)

signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
try:
    while True:
        signal.pause()
except (KeyboardInterrupt, SystemExit):
    x11.XCloseDisplay(display)
