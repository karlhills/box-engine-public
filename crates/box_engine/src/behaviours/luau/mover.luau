-- Mover, as a script: moves the entity through points, measured from where
-- it starts, and carries characters standing on it. Made by Convert to
-- Script from the Mover behaviour; each entity keeps its own values in the
-- Script's properties (up to eight points, `point_1` to `point_8`, of
-- which the first `point_count` are used). Change anything: this file is
-- yours now.

local Mover = {}

Mover.properties = {
    enabled = { type = "boolean", default = true, doc = "Off: it stops where it is" },
    speed = { type = "number", default = 2, min = 0, max = 100, unit = "m/s", doc = "Travel speed" },
    mode = { type = "string", default = "PingPong", doc = "PingPong (back and forth), Loop (round and round) or Once (stops at the end)" },
    wait = { type = "number", default = 0.5, min = 0, max = 60, unit = "s", doc = "Pause at each end" },
    point_count = { type = "integer", default = 2, min = 0, max = 8, doc = "How many of the points it uses" },
    point_1 = { type = "vector", default = vector.create(0, 0, 0), unit = "m", doc = "First point, relative to the start (usually 0, 0, 0)" },
    point_2 = { type = "vector", default = vector.create(4, 0, 0), unit = "m" },
    point_3 = { type = "vector", default = vector.create(0, 0, 0), unit = "m" },
    point_4 = { type = "vector", default = vector.create(0, 0, 0), unit = "m" },
    point_5 = { type = "vector", default = vector.create(0, 0, 0), unit = "m" },
    point_6 = { type = "vector", default = vector.create(0, 0, 0), unit = "m" },
    point_7 = { type = "vector", default = vector.create(0, 0, 0), unit = "m" },
    point_8 = { type = "vector", default = vector.create(0, 0, 0), unit = "m" },
}

-- The points of one pass (closed for Loop).
local function points(props): { vector }
    local out = {}
    for i = 1, math.clamp(props.point_count, 0, 8) do
        table.insert(out, props["point_" .. i])
    end
    if props.mode == "Loop" and #out > 0 then
        table.insert(out, out[1])
    end
    return out
end

local function pass_length(pts: { vector }): number
    local l = 0
    for i = 2, #pts do
        l += vector.magnitude(pts[i] - pts[i - 1])
    end
    return l
end

-- The offset at distance `s` along one pass.
local function offset_at(pts: { vector }, s: number): vector
    if #pts == 0 then
        return vector.create(0, 0, 0)
    end
    local left = math.max(s, 0)
    for i = 2, #pts do
        local d = vector.magnitude(pts[i] - pts[i - 1])
        if left <= d then
            if d > 0 then
                return pts[i - 1] + (pts[i] - pts[i - 1]) * (left / d)
            end
            return pts[i - 1]
        end
        left -= d
    end
    return pts[#pts]
end

function Mover.on_start(self)
    self.origin = self.entity:position()
    self.travelled = 0
    self.waiting = 0
    self.finished = false
end

-- Moves at the fixed rate, before characters move, so a character standing
-- on it is carried in the same step.
function Mover.on_fixed_update(self, dt: number)
    local p = self.props
    local pts = points(p)
    local len = pass_length(pts)
    local ping_pong = p.mode ~= "Loop" and p.mode ~= "Once"
    local cycle = if ping_pong then 2 * len else len
    if p.enabled and not self.finished and cycle > 0 and p.speed > 0 then
        local left = dt
        if self.waiting > 0 then
            local used = math.min(self.waiting, left)
            self.waiting -= used
            left -= used
        end
        local ends = if ping_pong then { len, 2 * len } else { len }
        while left > 0 and self.waiting <= 0 do
            local next_end = cycle
            for _, e in ends do
                if e > self.travelled + 1e-6 then
                    next_end = e
                    break
                end
            end
            local step = p.speed * left
            if self.travelled + step < next_end then
                self.travelled += step
                break
            end
            left -= (next_end - self.travelled) / p.speed
            self.travelled = next_end
            if p.mode == "Once" then
                self.finished = true
                break
            end
            if self.travelled >= cycle - 1e-6 then
                self.travelled = 0
            end
            self.waiting = p.wait
            if self.waiting > 0 then
                local used = math.min(self.waiting, left)
                self.waiting -= used
                left -= used
            end
        end
    end
    local s = self.travelled
    if ping_pong and s > len then
        s = 2 * len - s
    end
    self.entity:set_position(self.origin + offset_at(pts, s))
end

return Mover
