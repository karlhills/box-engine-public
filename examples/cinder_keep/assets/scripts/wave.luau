-- The courtyard wave, as one batch script: when the hero walks into the
-- courtyard it spawns `count` cinderlings around the arena edge and runs
-- all of them from Lua arrays (no script per enemy): cinderlings chase the
-- hero and burn on contact; every `spitter_every`-th one is a spitter that
-- keeps its distance and throws embers (a pool of projectile entities with
-- particle trails, hidden while unused, never spawned mid-fight). The hero's sweep
-- (`player_attack`) kills cinderlings in range with a burst of sparks and
-- scatters embers. When the last one dies the gate unlocks: its rigid body
-- turns dynamic and swings open on its hinge.
--
-- Enemies and embers are transient (runtime-only: never saved, removed
-- when a checkpoint loads); the wave's own state (waiting or cleared) is
-- saved, and a loaded wave that was not cleared waits again.

local util = require("./util")

local Wave = {}

Wave.properties = {
    count = { type = "integer", default = 120, min = 1, max = 1000, doc = "Cinderlings in the wave" },
    spitter_every = { type = "integer", default = 10, min = 0, max = 100, doc = "Every n-th cinderling is a spitter (0: none)" },
    centre = { type = "vector", default = vector.create(0, 2, -44), unit = "m", doc = "Arena centre; y is the ground height" },
    half_size = { type = "vector", default = vector.create(13.5, 0, 11.5), unit = "m", doc = "Arena half extents in x and z" },
    start_z = { type = "number", default = -35, unit = "m", doc = "The wave starts when the hero is north of this z" },
    gate = { type = "entity", doc = "Unlocked and swung open when the wave is defeated" },
    gate_spin = { type = "number", default = 2.2, min = 0, max = 10, unit = "rad/s", doc = "How fast the gate swings open" },
}

-- No `materials`: imported meshes draw with the materials they were
-- imported with.
local CINDERLING = { mesh = "models/props.glb#Mesh/Cinderling" }
local SPITTER = { mesh = "models/props.glb#Mesh/Spitter" }
local EMBER = { mesh = "models/props.glb#Mesh/Ember" }
local HOVER = 0.38

function Wave.on_start(self)
    self.state = "waiting"
    self.n = 0
    self.alive = 0
    self.reported = -1
    self.kills = 0
    self.fired = 0
    -- Enemy arrays (index i): entity, x, z, kind (true = spitter), phase,
    -- fire cooldown.
    self.ent, self.x, self.z, self.spit, self.phase, self.reload = {}, {}, {}, {}, {}, {}
    self.cells = {}
    -- The ember pool: entity, active, position, velocity, age.
    self.pool, self.live, self.pos, self.vel, self.age = {}, {}, {}, {}, {}
    self.fire_timer = 0
    self.tuning = util.wait_for_tuning()
    events.on("player_attack", function(ev)
        self:sweep(ev.at, ev.radius)
    end)
end

function Wave.start(self)
    local t = self.tuning
    local p = self.props
    local c = p.centre
    local hero = world.find("Player")
    local hero_at = if hero then hero:position() else c
    self.state = "fighting"
    log.info("the wave begins: " .. p.count .. " cinderlings")
    for i = 1, p.count do
        -- Around the arena edge, away from the hero.
        local x, z
        for _ = 1, 8 do
            local a = math.random() * 2 * math.pi
            local r = 0.75 + 0.25 * math.random()
            x = c.x + math.cos(a) * p.half_size.x * r
            z = c.z + math.sin(a) * p.half_size.z * r
            if (x - hero_at.x) ^ 2 + (z - hero_at.z) ^ 2 > 36 then
                break
            end
        end
        local spitter = p.spitter_every > 0 and i % p.spitter_every == 0
        local e = world.spawn({
            name = if spitter then "Spitter" else "Cinderling",
            Transform = { translation = vector.create(x, c.y + HOVER, z) },
            MeshAsset = if spitter then SPITTER else CINDERLING,
            transient = true,
        })
        self.ent[i] = e
        self.x[i] = x
        self.z[i] = z
        self.spit[i] = spitter
        self.phase[i] = math.random() * 2 * math.pi
        self.reload[i] = t.spitter.fire_interval[1] + math.random() * (t.spitter.fire_interval[2] - t.spitter.fire_interval[1])
    end
    self.n = p.count
    self.alive = p.count
    for k = 1, t.ember.pool do
        -- Hidden until fired: not drawn, and its trail is off.
        local e = world.spawn({
            name = "Ember",
            transient = true,
            Visible = false,
            MeshAsset = EMBER,
            ParticleEmitter = {
                rate = 40,
                max_particles = 64,
                lifetime = vector.create(0.2, 0.35),
                speed = vector.create(0.2, 0.6, 0),
                size = { { t = 0, value = 0.16 }, { t = 1, value = 0.02 } },
                color = {
                    { t = 0, color = Color.rgb(1.0, 0.6, 0.2, 1) },
                    { t = 1, color = Color.rgb(0.8, 0.1, 0.02, 0) },
                },
                intensity = 5,
                blend = "Additive",
                space = "World",
            },
        })
        self.pool[k] = e
        self.live[k] = false
        self.pos[k] = vector.create(0, 0, 0)
        self.vel[k] = vector.create(0, 0, 0)
        self.age[k] = 0
    end
    events.send("wave_started", { count = p.count })
end

-- Sparks where a cinderling dies.
local function death_burst(at: vector, spitter: boolean)
    local hot = if spitter then Color.rgb(0.9, 0.4, 1.0, 1) else Color.rgb(1.0, 0.65, 0.2, 1)
    particles.burst(at, 20, {
        speed = vector.create(2, 5, 0),
        lifetime = vector.create(0.3, 0.6),
        gravity = vector.create(0, -9, 0),
        size = { { t = 0, value = 0.2 }, { t = 1, value = 0.05 } },
        color = {
            { t = 0, color = hot },
            { t = 1, color = Color.rgb(0.6, 0.1, 0.02, 0) },
        },
        intensity = 5,
        blend = "Additive",
    })
end

-- The hero's sweep: kills every cinderling within `radius` of `at` and
-- scatters embers a little farther out.
function Wave.sweep(self, at: vector, radius: number)
    if self.state ~= "fighting" then
        return
    end
    local t = self.tuning
    local r2 = radius * radius
    for i = 1, self.n do
        local e = self.ent[i]
        if e ~= nil then
            local dx, dz = self.x[i] - at.x, self.z[i] - at.z
            if dx * dx + dz * dz <= r2 then
                death_burst(e:position(), self.spit[i])
                e:despawn()
                self.ent[i] = nil
                self.alive -= 1
                self.kills += 1
                events.send("enemy_killed", { points = if self.spit[i] then t.spitter.points else t.cinderling.points })
            end
        end
    end
    local deflect = (radius + 0.3) ^ 2
    for k = 1, #self.pool do
        if self.live[k] then
            local d = self.pos[k] - at
            if d.x * d.x + d.z * d.z <= deflect then
                self:park(k, true)
            end
        end
    end
    if self.alive == 0 then
        self:cleared()
    end
end

function Wave.park(self, k: number, burst: boolean)
    if burst then
        particles.burst(self.pos[k], 10, {
            speed = vector.create(1, 3, 0),
            lifetime = vector.create(0.15, 0.3),
            color = {
                { t = 0, color = Color.rgb(1.0, 0.7, 0.3, 1) },
                { t = 1, color = Color.rgb(1.0, 0.2, 0.0, 0) },
            },
            intensity = 4,
            blend = "Additive",
        })
    end
    self.live[k] = false
    -- Hidden: not drawn, and its trail is dropped.
    self.pool[k]:set_visible(false)
end

function Wave.fire(self, from: vector, target: vector)
    for k = 1, #self.pool do
        if not self.live[k] then
            local dir = target - from
            local len = vector.magnitude(dir)
            if len < 0.01 then
                return
            end
            self.live[k] = true
            self.pos[k] = from
            self.vel[k] = dir / len * self.tuning.ember.speed
            self.age[k] = 0
            self.fired += 1
            self.pool[k]:set_position(from)
            self.pool[k]:set_visible(true)
            return
        end
    end
end

function Wave.cleared(self)
    self.state = "cleared"
    log.info("the wave is defeated (" .. self.kills .. " kills)")
    for k = 1, #self.pool do
        self.pool[k]:despawn()
    end
    self.pool, self.live = {}, {}
    events.send("wave_cleared", {})
    local gate = self.props.gate
    if gate ~= nil and gate:is_alive() then
        gate:set("RigidBody", { kind = "Dynamic" })
        -- Swing it open: the body is dynamic from the next fixed step.
        task.delay(0.05, function()
            if gate:is_alive() then
                -- Turning about the hinge, not the centre: the centre moves
                -- too (v = w x r, r from the hinge to the centre).
                local w = vector.create(0, self.props.gate_spin, 0)
                local joint = gate:get("Joint")
                local arm = gate:rotation() * (-joint.anchor)
                local spin = { linear = vector.cross(w, arm), angular = w }
                if gate:has("Velocity") then
                    gate:set("Velocity", spin)
                else
                    gate:add("Velocity", spin)
                end
            end
        end)
    end
end

function Wave.on_update(self, dt: number)
    if self.tuning == nil or self.state == "cleared" then
        return
    end
    local hero = world.find("Player")
    if hero == nil then
        return
    end
    local h = hero:position()
    if self.state == "waiting" then
        if h.z < self.props.start_z and h.y > self.props.centre.y - 1 then
            self:start()
        end
        return
    end
    if dt <= 0 then
        return
    end
    local t = self.tuning
    local c = self.props.centre
    local half = self.props.half_size
    local ground = c.y + HOVER
    local now = time.elapsed()
    local n = self.n
    local ent, xs, zs, spit, phase, reload = self.ent, self.x, self.z, self.spit, self.phase, self.reload

    -- Neighbour grid (1 m cells) for spacing; the cell tables are reused.
    local cells = self.cells
    for _, list in cells do
        table.clear(list)
    end
    for i = 1, n do
        if ent[i] ~= nil then
            local key = math.floor(xs[i]) * 4096 + math.floor(zs[i])
            local list = cells[key]
            if list == nil then
                list = {}
                cells[key] = list
            end
            table.insert(list, i)
        end
    end

    local spacing = t.cinderling.spacing
    local spacing2 = spacing * spacing
    local contact2 = t.cinderling.contact_range ^ 2
    local touched = false
    self.fire_timer -= dt
    for i = 1, n do
        local e = ent[i]
        if e ~= nil then
            local x, z = xs[i], zs[i]
            local dx, dz = h.x - x, h.z - z
            local d2 = dx * dx + dz * dz
            local d = math.sqrt(d2)
            local speed
            if spit[i] then
                speed = if d > t.spitter.hold_distance then t.spitter.speed else 0
                reload[i] -= dt
                if reload[i] <= 0 and d < t.spitter.fire_range and self.fire_timer <= 0 then
                    reload[i] = t.spitter.fire_interval[1] + math.random() * (t.spitter.fire_interval[2] - t.spitter.fire_interval[1])
                    self.fire_timer = t.ember.min_interval
                    self:fire(vector.create(x, ground + 0.4, z), h + vector.create(0, 1.0, 0))
                end
            else
                speed = t.cinderling.speed
            end
            local mx, mz = 0, 0
            if d > 0.05 and speed > 0 then
                mx, mz = dx / d * speed * dt, dz / d * speed * dt
            end
            -- Keep apart from neighbours.
            local cx, cz = math.floor(x), math.floor(z)
            for ox = -1, 1 do
                for oz = -1, 1 do
                    local list = cells[(cx + ox) * 4096 + cz + oz]
                    if list ~= nil then
                        for _, j in list do
                            if j ~= i then
                                local sx, sz = x - xs[j], z - zs[j]
                                local s2 = sx * sx + sz * sz
                                if s2 < spacing2 and s2 > 1e-6 then
                                    local s = math.sqrt(s2)
                                    local push = (spacing - s) * 0.5 / s
                                    mx += sx * push
                                    mz += sz * push
                                end
                            end
                        end
                    end
                end
            end
            x = math.clamp(x + mx, c.x - half.x, c.x + half.x)
            z = math.clamp(z + mz, c.z - half.z, c.z + half.z)
            xs[i], zs[i] = x, z
            if not spit[i] and d2 < contact2 then
                touched = true
            end
            local hop = math.abs(math.sin(now * 9 + phase[i])) * 0.25
            e:set_position(vector.create(x, ground + hop, z))
            if d > 0.05 then
                e:set_rotation(Quat.from_rotation_y(util.yaw_of(vector.create(dx, 0, dz))))
            end
        end
    end
    if touched then
        events.send("damage", { amount = t.cinderling.contact_damage })
    end

    -- Embers.
    local chest = h + vector.create(0, 1.0, 0)
    local hit2 = t.ember.hit_radius ^ 2
    for k = 1, #self.pool do
        if self.live[k] then
            local p = self.pos[k] + self.vel[k] * dt
            self.pos[k] = p
            self.age[k] += dt
            local d = p - chest
            if d.x * d.x + d.y * d.y + d.z * d.z < hit2 then
                events.send("damage", { amount = t.ember.damage })
                self:park(k, true)
            elseif self.age[k] > t.ember.lifetime or p.y < c.y then
                self:park(k, false)
            else
                self.pool[k]:set_position(p)
            end
        end
    end

    if self.alive ~= self.reported then
        self.reported = self.alive
        events.send("wave_progress", { left = self.alive })
    end
end

function Wave.on_save(self)
    -- A save during the fight restarts it (the wave waits again).
    return { cleared = self.state == "cleared", kills = self.kills }
end

function Wave.on_load(self, saved)
    if saved.cleared then
        self.state = "cleared"
    end
    self.kills = saved.kills
end

return Wave
