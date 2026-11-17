-- A script rule: it turns an entity's ScriptIntent (a list of points plus
-- the properties below) into generated parts. This one lays a garden path
-- along the points: a gravel strip swept along them (Spline + Sweep),
-- stepping stones spaced along each stretch (Extrude) and a lantern beside
-- every point (Lathe).
--
-- Parts are runtime-only: never saved, rebuilt whenever the intent, the
-- properties or this file change, in the editor and in the game. Select
-- "Garden Path" in the editor and drag its points with the Path tool, or
-- change the properties in the inspector, and the path follows. Edit this
-- file while the editor is open and every path using it rebuilds.

local GardenPath = {}

-- Shown in the inspector of every entity whose ScriptIntent uses this rule.
GardenPath.properties = {
    width = { type = "number", default = 1.2, min = 0.4, max = 4, unit = "m", doc = "Width of the gravel strip" },
    spacing = { type = "number", default = 0.8, min = 0.3, max = 5, unit = "m", doc = "Distance between two stepping stones" },
    lanterns = { type = "boolean", default = true, doc = "A lantern beside every point" },
    stone = { type = "color", default = Color.srgb(0.62, 0.6, 0.56), doc = "Stepping stone colour" },
}

-- A path's parts depend only on its own points and properties, so editing
-- one path re-runs the rule for that path alone.
GardenPath.scope = "intent"

local UP = vector.create(0, 1, 0)

-- `intents` are the entities using this rule; `out:part(intent, key,
-- components, transform)` adds a generated part. The key names the part:
-- the same key next time updates it in place instead of replacing it.
function GardenPath.rule(intents: { RuleIntent }, out: RuleOut)
    for _, it in intents do
        local p = it.props
        local pts = it.points
        if #pts < 2 then
            out:warn(it, "a garden path needs at least two points")
            continue
        end

        -- The gravel: a flat rectangle swept along straight segments between
        -- the points (Polyline), so the stones below lie on it.
        local path = {}
        for i, q in pts do
            path[i] = { position = q }
        end
        out:part(it, "gravel", {
            Spline = { points = path, mode = "Polyline" },
            Sweep = { profile = { Rect = { width = p.width, height = 0.04 } }, orientation = "Up" },
            StandardMaterial = { base_color = Color.srgb(0.78, 0.74, 0.66), roughness = 1 },
        })

        -- Stepping stones: hexagonal slabs, evenly spaced along each stretch.
        local stone = {
            Extrude = {
                outline = { Polygon = { sides = 6, radius = p.width * 0.3 } },
                depth = 0.06,
                bevel_width = 0.015,
            },
            StandardMaterial = { base_color = p.stone, roughness = 0.9 },
        }
        for i = 1, #pts - 1 do
            local d = pts[i + 1] - pts[i]
            local count = math.max(1, math.floor(vector.magnitude(d) / p.spacing))
            for k = 0, count - 1 do
                out:part(it, `leg{i}/stone{k}`, stone, {
                    position = pts[i] + d * ((k + 0.5) / count) + UP * 0.02,
                    rotation = Quat.from_rotation_y(k * 0.9),
                })
            end
        end

        -- A glowing lantern beside every point, on the left of the path.
        if p.lanterns then
            for i, q in pts do
                local d = pts[math.min(i + 1, #pts)] - pts[math.max(i - 1, 1)]
                local side = vector.normalize(vector.cross(d, UP))
                out:part(it, `lantern{i}`, {
                    Lathe = { shape = { Vase = { height = 0.8, radius = 0.16 } }, segments = 16 },
                    StandardMaterial = { base_color = Color.srgb(0.3, 0.32, 0.36), emissive = Color.srgb(0.9, 0.6, 0.25) },
                }, { position = q + side * (p.width / 2 + 0.4) })
            end
        end
    end
end

return GardenPath
