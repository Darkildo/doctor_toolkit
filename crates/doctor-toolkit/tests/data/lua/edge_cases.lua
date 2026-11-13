-- Less common code shapes of Lua 5.0.
local a, b, c, d
local t = {}
a, t[a], t.x = 1, 2, 3
t[a], a = a, t[a]
local u, v = 1
local w = 1, 2, 3
x, y, z = f()
local p, q, r = g(), h()
local s = (g())
print((g()), g())

local function outer(n, ...)
  local count = 0
  local function inner(m)
    local function deepest()
      count = count + n + m
      return arg, count
    end
    return deepest
  end
  return inner(n)
end

local list = {
  1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
  26, 27, 28, 29, 30, 31, 32, 33, 34, name = "x", 35, ["key with space"] = true, f(),
}
local open = { g() }
local closed = { (g()) }

while a and (b or c) and not d do
  a = a - 1
end
while not a or b == c do break end
repeat
  local inside = a
  a = function() return inside end
until a or b and c
for i = 0.5, 10.25, 0.75 do
  if i > 5 then break elseif i < 1 then a = i else b = i end
end
for i = 10, 1, -1 do
  local captured = i
  t[i] = function() return captured end
end
if a then
  if b then c = 1 else c = 2 end
elseif b then
  c = 3
end
if a then
  if b then c = 1 end
else
  c = 2
end
local value = a and b or c
local cmp = a < b and b <= c or c > d and d >= a
local eq = (a == b) == (c ~= d)
local n = not not a
local k = - -a
local e = 2 ^ -3 + (-2) ^ 3 - -2 ^ 2
local str = "esc\a\b\f\n\r\t\v\\\"\'\0\255" .. 'single' .. [[long
string]]
local method = ("x"):rep(3):upper()
t.a.b.c = t["a"]["b"].d
do
  local hidden = 1
  local function capture() return hidden end
  t.capture = capture
end
return a, b, f()
