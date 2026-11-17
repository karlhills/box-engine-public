-- Input-driven movement: the `move` action (WASD, arrows, left stick)
-- moves the player on the ground relative to the camera; `jump` hops.
-- Gravity and ground contact come from a ray cast down each fixed step.

local util = require("./util")

local Player = {}

Player.properties = {
    speed = { type = "number", default = 5, min = 0, max = 20, unit = "m/s", doc = "Walking speed" },
    jump_speed = { type = "number", default = 6, min = 0, max = 20, unit = "m/s" },
    gravity = { type = "number", default = 20, min = 0, max = 60, unit = "m/s²" },
    camera = { type = "entity", doc = "The camera the movement is relative to" },
}

local RADIUS = 0.5

function Player.on_start(self)
    self.vy = 0
    self.start = self.entity:position()
end

-- The camera's heading (0 looks down -Z).
local function view_yaw(camera: Entity?): number
    if camera == nil then
        return 0
    end
    local f = camera:forward()
    return math.atan2(-f.x, -f.z)
end

function Player.on_fixed_update(self, dt: number)
    local wish = util.wish(input.axis2("move"), view_yaw(self.props.camera))
    local step = wish * self.props.speed * dt
    local at = self.entity:position()
    local ground = physics.raycast(at, vector.create(0, -1, 0), 50, { exclude = self.entity })
    local grounded = ground ~= nil and ground.distance <= RADIUS + 0.05
    if grounded and self.vy <= 0 then
        self.vy = 0
        step += vector.create(0, RADIUS - ground.distance, 0)
        if input.pressed("jump") then
            self.vy = self.props.jump_speed
        end
    else
        self.vy -= self.props.gravity * dt
    end
    step += vector.create(0, self.vy * dt, 0)
    self.entity:translate(step)
    if at.y < -20 then
        self.entity:set_position(self.start)
        self.vy = 0
    end
end

return Player
