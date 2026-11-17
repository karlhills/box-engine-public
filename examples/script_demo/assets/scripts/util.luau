-- Shared helpers, loaded with require("./util").

local util = {}

-- A smooth up-and-down offset in -1..1, out of step per position.
function util.bob(t: number, at: vector): number
    return math.sin(t * 2.2 + at.x * 0.7 + at.z * 1.3)
end

-- The horizontal direction for a move input (x right, y forward) seen
-- from a yaw angle (0 looks down -Z).
function util.wish(input: vector, yaw: number): vector
    local forward = vector.create(-math.sin(yaw), 0, -math.cos(yaw))
    local right = vector.create(math.cos(yaw), 0, -math.sin(yaw))
    local dir = right * input.x + forward * input.y
    local len = vector.magnitude(dir)
    if len > 1 then
        dir /= len
    end
    return dir
end

return util
