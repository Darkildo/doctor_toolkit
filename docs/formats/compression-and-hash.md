# Формат: сжатие (RefPack + LZX) и хэширование (SDBM)

## 1. RefPack (сжатие секций Stream)

**Проверено на GF2 PC**: 146 544 блока `Rpak` распакованы в заявленный размер.
Диапазоны длин и смещений длинного матча и литерального прогона исправлены.

Проприетарный LZ-вариант EA. Используется для сжатых секций RwStream (Godfather)
и блоков `Rpak` в UStream (Dead Space). Сигнатура (логическая):

```
int refpack_decompress(in: src[], in_size, out: dst[], out_size,
                       out: flags(24bit), out: size(24bit), out: bytes_written)
```

Возвращает: `0` — успех; `-1` (`0xFFFFFFFF`) — вход обрезан (недостаточно данных);
`-2` (`0xFFFFFFFE`) — выходной буфер переполнен.

### 1.1 Заголовок

Порядок байт — **big-endian** для многобайтовых значений заголовка.

| Смещение | Размер | Поле |
|---|---|---|
| +0x00 | 1 | `flags_byte` (бит 0 = признак расширенного заголовка) |
| +0x01 | 1 | (не читается как часть значения) |
| +0x02 | 3 | `flags` (24-bit BE) — **только если** `flags_byte & 1` |
| далее | 3 | `size` (24-bit BE) — всегда присутствует |

Алгоритм разбора заголовка:

```
if in_size > 1:
    if (flags_byte & 1) == 0:
        header_len = 2; flags = 0
    else:
        header_len = 5; flags = read_u24_be(in + 2)
else:
    error(-1)
size = read_u24_be(in + header_len)
pos = header_len + 3
```

`flags` и `size` — это два 24-битных значения, возвращаемых наружу (размеры
сжатого/несжатого блока в обёртке секции; сам декомпрессор их не использует
для распаковки).

### 1.2 Команды (opcode)

Каждая команда начинается с 1-байтового опкода `op`. Дальнейшее зависит от
диапазона `op`. Копирование «матча» всегда идёт перекрывающимся `memcpy`-стилем
(байт за байтом назад от текущей позиции вывода на `offset` байт).

**A. `op < 0x80` (короткий матч), всего 2 байта: `[op][lo]`**

| Поле | Биты | Значение |
|---|---|---|
| `num_lit` | `op & 0x03` | литералы, 0–3 |
| `len` | `(op >> 2) & 0x07` | длина матча = `len + 3` → 3–10 |
| `off_hi` | `op & 0x60` | старшие биты смещения |
| `offset` | `lo | (off_hi << 3)` | смещение = `offset + 1` → 1–1024 |

```
copy literal num_lit bytes
copy match: len+3 bytes from (out - (offset+1))
```

**B. `0x80 <= op < 0xC0` (средний матч), 3 байта: `[op][b1][b2]`**

| Поле | Значение |
|---|---|
| `num_lit` | `b1 >> 6` → 0–3 |
| `offset` | `b2 + ((b1 & 0x3F) << 8) + 1` → 1–16384 |
| `len` | `(op & 0x3F) + 4` → 4–67 |

```
copy literal num_lit bytes
copy match: len bytes from (out - offset)
```

**C. `0xC0 <= op < 0xE0` (длинный матч), 4 байта: `[op][b1][b2][b3]`**

| Поле | Значение |
|---|---|
| `num_lit` | `op & 0x03` → 0–3 |
| `offset` | `(((op & 0x10) << 4 | b1) << 8 | b2) + 1` → 1–131072 |
| `len` | `b3 + 5 + (op & 0x0C) * 0x40` → 5–1028 |

```
copy literal num_lit bytes
copy match: len bytes from (out - offset)
```

**D. `0xE0 <= op < 0xFC` (литеральный прогон)**

| Поле | Значение |
|---|---|
| `len` | `(op & 0x1F) * 4 + 4` → 4–112 (опкоды `0xE0`–`0xFB`) |

```
copy literal len bytes
```

**E. `0xFC <= op <= 0xFF` (короткий литеральный прогон)**

| Поле | Значение |
|---|---|
| `len` | `op & 0x03` → 0–3 |

```
copy literal len bytes
```

### 1.3 Флаг секции

Секция Stream сжата RefPack, если её поле `+0x04` == `0xB9F0B9EC`
(также встречается как `-0x460F4614`). Иначе секция хранится сырой.

---

## 2. LZX (второй декодер)

Это **настоящий Microsoft LZX** (формат CAB), а не вариант RefPack. Поток состоит
из блоков; 16-битные значения — **big-endian**.

### 2.1 Блочный reader

```
while remaining > 0:
    b0 = read byte
    if b0 == 0xFF:                       # блок с расширенным заголовком (5 байт)
        out_size   = read_u16_be()       # bytes[1..2]
        block_size = read_u16_be()       # bytes[3..4]
        data = read(block_size)          # смещение 5
    else:                                # обычный блок (2 байта)
        out_size   = 0x8000              # фикс. (32 КБ)
        block_size = read_u16_be()       # bytes[0..1]
        data = read(block_size)          # смещение 2
    decode_block(out_size, data, block_size, out_buf, ...)
```

Старший байт 16-битного размера `0xFF` служит маркером «расширенного» блока
(явный размер вывода), обычный блок декодируется до 32 КБ вывода.

### 2.2 Заголовок LZX-блока

Первый бит блока:

- `read_bits(1) == 0` → **несжатый блок**: сырые данные.
- `read_bits(1) == 1` → **сжатый блок**: `read_bits(16)` + `read_bits(16)` = 32-битный
  размер сжатого блока.

Далее для сжатого блока:

| Поле | Биты | Значение |
|---|---|---|
| `block_type` | `read_bits(3)` | 1 = verbatim, 2 = aligned-offset, 3 = uncompressed |
| `block_size` | `read_bits(8)*3` | 24-bit, оставшийся размер блока |

По `block_type`:

- **type 2** — читается дерево «aligned offset», затем основное дерево;
  декодирование символов.
- **type 1** — основное дерево + length-дерево, декодирование символов.
- **type 3** — uncompressed, прямое чтение.

Это в точности соответствует трём типам блоков Microsoft LZX (включая окно
декодера и E8-преобразование).

**Вывод:** LZX — настоящий Microsoft LZX (формат CAB), отдельный от RefPack кодек;
маркер `0xFF` — блок с явным размером вывода.

### 2.3 Потребитель LZX

**Потребитель LZX — бэкенд UStream (Dead Space).** Содержимое секций UStream
разбирает блоки секции (см. `stream.md` §6.3–6.5) и для блока с subtype
`LZXD` (`0x4C5A5844`) вызывает LZX-декодер, а для блока `Rpak` (`0x5270616B`) —
RefPack-декодер. Логика:

```
для каждого блока секции (type == "SHOC"):
  if subtype == "LZXD":
      read u32 compressed_size
      read compressed_size байт (LZX-поток)
      LZX-декодировать в выходной буфер (≤ 0x7FFF8 байт)
  elif subtype == "Rpak":
      read u32 compressed_size
      read compressed_size байт (RefPack-поток)
      RefPack-декодировать
  elif subtype == "SDAT":
      сырые данные
  данные секции = конкатенация распакованных блоков
```

То есть LZX используется **только** для сжатых секций UStream/Dead Space (блок
`LZXD`); RwStream/Godfather использует RefPack (флаг секции `0xB9F0B9EC`).

---

## 3. SDBM (хэш имён Godfather)

Алгоритм стандартный:

```
hash = 0
for c in name:
    hash = c + (hash << 6) + (hash << 16) - hash   # == c + hash * 65599
return u32(hash)
```

Применяется к именам ресурсов/событий (напр. `ShaderNameID`, `EventId`).
Для Dead Space хэши иные (32-битный, не SDBM).
