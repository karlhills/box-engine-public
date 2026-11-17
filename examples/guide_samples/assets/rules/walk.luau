-- A garden walk along the intent's points: a gravel strip swept along the
-- path, stepping stones spaced along it and a lantern beside every point.
-- The sample rule of docs/guide/procedural.md.

local Walk = {}

Walk.properties = {
    width = { type = "number", default = 1.2, min = 0.4, max = 4, unit = "m", doc = "Width of the gravel strip" },
    spacing = { type = "number", default = 0.8, min = 0.3, max = 5, unit = "m", doc = "Distance between two stones" },
    lanterns = { type = "boolean", default = true, doc = "A lantern beside every point" },
    stone = { type = "color", default = Color.srgb(0.62, 0.6, 0.56), doc = "Stone colour" },
}

-- A walk's parts depend only on its own points and properties, so an edit
-- re-runs the rule for the edited walk alone.
Walk.scope = "intent"

local UP = vector.create(0, 1, 0)

function Walk.rule(intents: { RuleIntent }, out: RuleOut)
    for _, it in intents do
        local p = it.props
        local pts = it.points
        if #pts < 2 then
            out:warn(it, "a walk needs at least two points")
            continue
        end

        -- The gravel: one sweep along the whole path.
        local path = {}
        for i, q in pts do
            path[i] = { position = q }
        end
        out:part(it, "gravel", {
            Spline = { points = path, mode = "Polyline" },
            Sweep = { profile = { Rect = { width = p.width, height = 0.04 } }, orientation = "Up" },
            StandardMaterial = { base_color = Color.srgb(0.78, 0.74, 0.66), roughness = 1 },
        })

        -- Stepping stones, evenly spaced along each stretch.
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

        -- A lantern (the built-in Lathe generator) beside every point.
        if p.lanterns then
            for i, q in pts do
                -- The path's direction at this point, from its neighbours.
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

return Walk
