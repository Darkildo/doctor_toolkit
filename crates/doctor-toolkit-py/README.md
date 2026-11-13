# doctor-toolkit (Python)

Python bindings of [doctor_toolkit](https://github.com/Darkildo/doctor_toolkit):
read, convert and edit the game files of the EARS engine (The Godfather II PC)
from scripts and notebooks.

```sh
pip install maturin
maturin develop --release          # in this directory, inside a virtualenv
```

```python
import doctor_toolkit as dt

game = dt.Game("/path/to/The Godfather II")
for texture in game.stream("gf2/frontend").textures():
    open(f"{texture.name}.png", "wb").write(texture.to_png())
```

See `docs/python.md` in the repository (Russian) and the type stubs
`doctor_toolkit/_native.pyi`.
