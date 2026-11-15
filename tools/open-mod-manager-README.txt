Open Mod Manager — Powered by Doctor
====================================

A mod manager for The Godfather II (PC), part of doctor_toolkit
(https://github.com/Darkildo/doctor_toolkit). Free software under
GPL-3.0-only; see LICENSE.

Start
-----
Windows: open-mod-manager.exe (needs the Microsoft Visual C++ Redistributable
         2015-2022, x64, which most PCs already have)
Linux:   ./open-mod-manager
macOS:   ./open-mod-manager (the program is not signed: allow it in
         System Settings > Privacy & Security, or run
         xattr -d com.apple.quarantine open-mod-manager)

No installation is needed; the program can live in any folder. It finds the
installed game by itself, or asks for the game folder.

Use
---
1. Install mod... (or drop a .zip or .vpat file on the window).
2. Turn the mods you want on, drag the cards into the order you like
   (when two mods change the same thing, the lower one wins).
3. Write to game. Restore originals puts the original files back.

Close the game before Write to game or Restore originals. Every changed
file is backed up in _doctor_toolkit inside the game folder.

Play with mods starts the game with the mods without writing them into the
game folder; it needs the full doctor_toolkit package (doctor_live.asi) and
an ASI loader (dinput8.dll) in the game folder, and works with the
1.0.789.0 edition of the game.

Community: https://discord.gg/MatbKKdFR
This program contains no game data and is not affiliated with Electronic Arts.
