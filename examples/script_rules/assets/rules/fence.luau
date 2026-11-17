-- A fence along the intent's points: a post at every point and evenly
-- spaced posts between them, rails swept along the whole path.
--
-- Rules are pure functions of their intents (docs/SCRIPTING.md, "Script
-- rules"). This one also shows neighbour facts: where the ends of two
-- fences meet, only the fence with the lower uid builds the corner post,
-- and its `scope` says how far those facts reach.

local Fence = {}

Fence.properties = {
    spacing = { type = "number", default = 2.0, min = 0.5, max = 10, unit = "m", doc = "Largest distance between two posts" },
    height = { type = "number", default = 1.1, min = 0.3, max = 4, unit = "m", doc = "Height of the posts" },
    post_size = { type = "number", default = 0.12, min = 0.04, max = 0.5, unit = "m", doc = "Width of a square post" },
    rails = { type = "integer", default = 2, min = 0, max = 6, doc = "Rails between the posts" },
    color = { type = "color", default = Color.srgb(0.55, 0.38, 0.24), doc = "Wood colour" },
}

-- Ends closer than this (metres) share one post.
local SNAP = 0.1

-- A fence's parts depend only on itself and the fences with an end within
-- SNAP of its own, so an edit runs the rule for the edited fence and the
-- fences near it (old and new place), not for every fence.
Fence.scope = { neighbours = SNAP }

local function wood(color)
    return { base_color = color, roughness = 0.85 }
end

local function up(y: number): vector
    return vector.create(0, y, 0)
end

-- Fence ends in a grid of SNAP-sized cells (a spatial hash), so finding
-- the ends near a point costs the same with ten fences or a thousand.
local function cell(x: number, z: number): string
    return `{x},{z}`
end

local function nearby(ends, w: vector)
    local cx, cz = math.floor(w.x / SNAP), math.floor(w.z / SNAP)
    local found = {}
    for dx = -1, 1 do
        for dz = -1, 1 do
            for _, e in ends[cell(cx + dx, cz + dz)] or {} do
                if vector.magnitude(e.at - w) < SNAP then
                    table.insert(found, e)
                end
            end
        end
    end
    return found
end

-- Every fence end (world position) with its fence's uid.
local function fence_ends(intents: { RuleIntent })
    local ends = {}
    for _, it in intents do
        local n = #it.points
        if n >= 2 then
            for _, i in { 1, n } do
                local w = it:to_world(it.points[i])
                local key = cell(math.floor(w.x / SNAP), math.floor(w.z / SNAP))
                ends[key] = ends[key] or {}
                table.insert(ends[key], { at = w, uid = it.uid })
            end
        end
    end
    return ends
end

-- A fence builds the post at its end unless a fence with a lower uid ends
-- there too.
local function builds_post(ends, it: RuleIntent, p: vector): boolean
    for _, e in nearby(ends, it:to_world(p)) do
        if e.uid < it.uid then
            return false
        end
    end
    return true
end

function Fence.rule(intents: { RuleIntent }, out: RuleOut)
    local ends = fence_ends(intents)
    for _, it in intents do
        local p = it.props
        local pts = it.points
        if #pts < 2 then
            out:warn(it, "a fence needs at least two points")
            continue
        end
        local post = {
            Primitive = { shape = { Cuboid = { size = vector.create(p.post_size, p.height, p.post_size) } } },
            StandardMaterial = wood(p.color),
        }
        -- A post at every point (shared ends: built once).
        for i, q in pts do
            local last = i == 1 or i == #pts
            if not last or builds_post(ends, it, q) then
                out:part(it, "corner" .. i, post, { position = q + up(p.height / 2) })
            end
        end
        -- Evenly spaced posts between the points.
        for i = 1, #pts - 1 do
            local a, b = pts[i], pts[i + 1]
            local length = vector.magnitude(b - a)
            if length < p.post_size then
                out:warn(it, `points {i} and {i + 1} are closer than a post is wide`)
            end
            local bays = math.max(1, math.ceil(length / p.spacing - 1e-4))
            for k = 1, bays - 1 do
                local at = a + (b - a) * (k / bays)
                out:part(it, `bay{i}/post{k}`, post, { position = at + up(p.height / 2) })
            end
        end
        -- Rails: one sweep each along the whole path.
        local rail = p.post_size * 0.45
        for r = 1, p.rails do
            local y = p.height * r / (p.rails + 1) - rail / 2
            local points = {}
            for i, q in pts do
                points[i] = { position = q + up(y) }
            end
            out:part(it, "rail" .. r, {
                Spline = { points = points, mode = "Polyline", closed = false },
                Sweep = { profile = { Rect = { width = rail * 0.6, height = rail } }, orientation = "Up", caps = true },
                StandardMaterial = wood(p.color),
            })
        end
    end
end

return Fence
