-- Lua 5.0 constructs for the compiler and decompiler tests.
local kFlag = false
local kCount, kScale = 3, 0.2
local a, b, c = 1, 2
local t = { 1, 2, 3, x = 4, ["y z"] = 5, [10] = 6; 7 }
local big = { 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
  21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35 }
local nested = { a = { b = { c = "deep" } }, list = { { 1 }, { 2 } } }

gValue = -2
gText = "tab\tnew\nline \"quoted\" \\ \001 end"
gLong = [[
long string with ]] .. "concat"
gNum = 1e10 + 0.1 - 3.5e-3 * 2 / 7 ^ 2 ^ -1

function globalFunc(x, y, ...)
  local n = arg.n
  if x == nil then
    return y
  elseif x > y and y >= 0 or not kFlag then
    return x, y
  else
    return (select(n, unpack(arg)))
  end
end

local function fact(n)
  if n <= 1 then return 1 end
  return n * fact(n - 1)
end

local obj = {}
function obj:method(v)
  self.value = v or self.value or 0
  return self
end

function obj.static(p) return p .. "!" end

local function counter()
  local count = 0
  return function()
    count = count + 1
    return count
  end
end

for i = 1, 10 do
  if i == 5 then break end
  a = a + i
end
for i = 10, 1, -2 do b = b + i end
for k, v in pairs(t) do
  if type(v) == "number" then c = (c or 0) + v end
end
for k in next, t do a = a end
while a < 100 do
  a = a * 2
  if a == 64 then break end
end
while true do
  b = b - 1
  if b < 0 then break end
end
repeat
  local r = a
  a = r - 1
until a < 10 or kFlag

do
  local hidden = 5
  gGetHidden = function() return hidden end
end

local x, y = globalFunc(1, 2)
x, y = y, x
t.x, t[1] = t[1], t.x
local s = "a" .. "b" .. "c" .. x
local p = (x + y) * (x - y)
local q = -x ^ 2
local r = (-x) ^ 2
local cmp = x < y
local both = x and y
local either = x or y
local neg = not x
local mix = (x or 1) == (y and 2)
local m = obj:method(3):method(4).value
obj.static "str"
print({} == nil, s, p, q, r, cmp, both, either, neg, mix, m)
gCounter = counter()
gCounter()
print(fact(5), (fact(3)))
return t
