-- Small helpers shared by player.luau and camera.luau, loaded with
-- require("./util"). Angles are radians; a yaw of 0 looks down -Z (the
-- engine's forward), positive yaw turns left.

local util = {}

-- The horizontal world direction for a `move` value (x right, y forward)
-- seen from a camera with this yaw, at most length 1.
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

-- The yaw of a direction.
function util.yaw_of(dir: vector): number
    if math.abs(dir.x) + math.abs(dir.z) < 1e-4 then
        return 0
    end
    return math.atan2(-dir.x, -dir.z)
end

-- `from` turned toward `to` by at most `step`, the short way round.
function util.turn_towards(from: number, to: number, step: number): number
    local diff = (to - from + math.pi) % (2 * math.pi) - math.pi
    if math.abs(diff) <= step then
        return to
    end
    return from + math.sign(diff) * step
end

return util
