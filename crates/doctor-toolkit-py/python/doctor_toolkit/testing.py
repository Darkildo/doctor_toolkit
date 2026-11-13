"""A miniature synthetic game for tests and examples.

``build_sample_game(root)`` writes, with the library's own writers:

* ``gf2/level.str``: a 4x4 red DXT1 texture ``wall`` (``tg4h`` + ``tg4d``),
  a string table (``Hello``, ``World``, ``Don Corleone``), a SimGroup with two
  ``Entity`` objects (``don_vito`` at (1, 2, 3), ``barrel_01`` at
  (-4, 0.5, 10)) and an unparsed ``vfx`` resource;
* ``gf2/text/abcd.str``: a string table (``Yes``, ``No``);
* ``gf2/level.toc`` listing both streams, and a fake ``godfather2.exe``.

It returns a dict of the resource keys (``texture_header``, ``texture_data``,
``strings``, ``simgroup``, ``vfx``, ``text``) and ``model_id``.
"""

from ._native import build_sample_game

__all__ = ["build_sample_game"]
