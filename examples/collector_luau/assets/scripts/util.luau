-- Shared helpers, loaded with require("./util").

local util = {}

-- The horizontal world direction for a `move` value (x right, y forward)
-- under a view yaw (0 looks down -Z), at most length 1.
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

-- The heading of a direction (0 looks down -Z).
function util.yaw_of(dir: vector): number
    if math.abs(dir.x) + math.abs(dir.z) < 1e-4 then
        return 0
    end
    return math.atan2(-dir.x, -dir.z)
end

-- `from` turned toward `to` (radians) by at most `step`, the short way.
function util.turn_towards(from: number, to: number, step: number): number
    local diff = (to - from + math.pi) % (2 * math.pi) - math.pi
    if math.abs(diff) <= step then
        return to
    end
    return from + math.sign(diff) * step
end

-- How many gems of the current arena are still there.
function util.gems_left(): number
    local n = 0
    for _, gem in world.scripts("scripts/gem.luau") do
        if gem.entity:is_alive() then
            n += 1
        end
    end
    return n
end

return util
