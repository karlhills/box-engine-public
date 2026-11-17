-- A third-person camera that follows the target and orbits with the `look`
-- action (right stick) or the mouse while the cursor is locked (click to
-- lock; opening the pause menu releases it). It pulls in when a wall is
-- between the target and the camera. Its saved transform sets the
-- starting view direction; the player moves relative to `self.yaw`.

local util = require("./util")

local Camera = {}

Camera.properties = {
    target = { type = "entity", doc = "What to follow" },
    distance = { type = "number", default = 8, min = 1, max = 50, unit = "m", doc = "Distance from the point it looks at" },
    height = { type = "number", default = 1, min = -2, max = 10, unit = "m", doc = "Height of that point above the target" },
    pitch = { type = "number", default = 0.4, min = -0.5, max = 1.4, unit = "rad", doc = "Starting downward angle" },
    smoothing = { type = "number", default = 10, min = 0.5, max = 50, unit = "1/s", doc = "How quickly it catches up" },
    look_speed = { type = "number", default = 2.5, min = 0, max = 10, unit = "rad/s", doc = "Turn rate at full stick" },
    mouse_sensitivity = { type = "number", default = 0.004, min = 0, max = 0.05, step = 0.0005, unit = "rad", doc = "Turn per unit of mouse motion" },
}

function Camera.on_start(self)
    self.yaw = util.yaw_of(self.entity:forward())
    self.pitch = self.props.pitch
    self.fresh = true
end

function Camera.on_update(self, dt: number)
    if input.mouse_pressed("Left") and not ui.captures_input() then
        input.lock_cursor()
    end
    local target: Entity? = self.props.target
    if target == nil or not target:is_alive() then
        return
    end
    local p = self.props
    local stick = input.axis2("look")
    local mouse = if input.is_cursor_locked() then input.mouse_delta() else vector.create(0, 0, 0)
    self.yaw -= stick.x * p.look_speed * dt + mouse.x * p.mouse_sensitivity
    self.pitch = math.clamp(self.pitch - stick.y * p.look_speed * dt + mouse.y * p.mouse_sensitivity, -0.3, 1.3)
    -- Where the target is drawn this frame, so the camera moves as smoothly.
    local focus = target:interpolated_position() + vector.create(0, p.height, 0)
    local rotation = Quat.from_rotation_y(self.yaw) * Quat.from_rotation_x(-self.pitch)
    local back = rotation * vector.create(0, 0, 1)
    local hit = physics.raycast(focus, back, p.distance, { exclude = target })
    local distance = if hit then math.max(hit.distance - 0.3, 0.5) else p.distance
    local desired = focus + back * distance
    local ease = if self.fresh then 1 else 1 - math.exp(-p.smoothing * dt)
    self.fresh = false
    local at = self.entity:position()
    local pos = at + (desired - at) * ease
    self.entity:set_position(pos)
    local look = focus - pos
    if vector.magnitude(look) > 0.01 then
        self.entity:set_rotation(Quat.look_to(look))
    else
        self.entity:set_rotation(rotation)
    end
end

return Camera
