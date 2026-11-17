-- Shared helpers, loaded with `require("./util")` (scripting guide).
local util = {}

-- The point `i` of `count` points spread evenly on a circle.
function util.ring(count: number, radius: number, i: number): vector
    local a = (i / count) * math.pi * 2
    return vector.create(math.cos(a) * radius, 0, math.sin(a) * radius)
end

return util
