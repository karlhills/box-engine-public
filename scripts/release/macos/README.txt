box-engine @VERSION@ for macOS (Apple Silicon)
==============================================

A 3D game engine with an editor. Make games with Luau scripts; no Rust or
other toolchain is needed to build, play or export them.

What is here
------------

  Box Editor.app   the editor. It also holds the command-line tools and the
                   player that exported games are built from.
  bin/             box (command-line tool), box-editor, box-player: links
                   into Box Editor.app, usable without installing
  install.sh       installs the app and the commands (below)
  docs/            the guides (start with docs/guide/getting-started.md) and
                   the script API reference (docs/reference/README.md)
  examples/        example projects: open one with box-editor examples/<name>
  LICENSE.txt      MIT

Install
-------

In Terminal, in this folder:

  ./install.sh

This copies Box Editor.app to ~/Applications and links box, box-editor and
box-player into ~/.local/bin. No administrator password is needed. If
~/.local/bin is not on your PATH, the script prints the line to add to
~/.zprofile. Options: --app-dir /Applications installs the app for every user
(if you may write there); --bin-dir <folder> puts the links elsewhere.

The release is ad-hoc signed, not notarised. install.sh removes the download
quarantine from the installed app so macOS opens it. If you copy the app by
hand instead, right-click it and choose Open the first time.

Check:

  box --version
  box-editor --version
  box-player --version

First steps
-----------

  box new my-game            create a project
  box-editor my-game         open it in the editor (or open Box Editor from
                             Applications and choose Open Project)
  box run my-game            play it in a window
  box export --project my-game --target macos --out my-game/export
                             make my-game/export/my-game.app, a game that runs
                             on Macs without box-engine

docs/guide/getting-started.md walks through these steps, a first script and
the export. Exporting for Windows needs the Windows player template in
Box Editor.app/Contents/Resources/templates/windows; this release includes
it only if the folder exists (see the Windows guide when available).

Uninstall
---------

  ./install.sh --uninstall

or remove ~/Applications/Box Editor.app and the links box, box-editor and
box-player in ~/.local/bin. Your projects are never touched. The editor keeps
recent projects in ~/Library/Application Support/box-editor and its log in
~/Library/Logs/box-editor.log; delete them too if you like.
