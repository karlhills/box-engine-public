-- Shared helpers, loaded with require("./util").

local util = {}

util.TUNING = "data/tuning.data.json"

-- The tuning data table (assets/data/tuning.data.json, hot-reloaded); nil
-- until it has been imported: scripts wait for it in on_start.
function util.tuning(): any
    return data.table(util.TUNING)
end

-- Waits (in on_start or a task) until the tuning table is loaded.
function util.wait_for_tuning(): any
    wait_until(function()
        return data.table(util.TUNING) ~= nil
    end)
    return data.table(util.TUNING)
end

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

-- Whether `other` is the player's character.
function util.is_player(other: Entity): boolean
    return other:name() == "Player"
end

-- A comma-separated list as an array of trimmed strings.
function util.list(text: string): { string }
    local out = {}
    for _, item in string.split(text, ",") do
        local trimmed = string.gsub(item, "^%s*(.-)%s*$", "%1")
        if trimmed ~= "" then
            table.insert(out, trimmed)
        end
    end
    return out
end

return util
