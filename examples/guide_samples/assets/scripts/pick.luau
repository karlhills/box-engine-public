-- Picking with the mouse and finding where a point is drawn (reference: camera).
local Pick = {}

function Pick.on_start(self)
    self.ball = world.find("Ball")
    self.ball_visible = false
end

function Pick.on_update(self, dt)
    -- A click picks what is under the cursor.
    if input.mouse_pressed("Left") then
        local origin, direction = camera.cursor_ray()
        if origin and direction then
            local hit = physics.raycast(origin, direction, 100)
            if hit then
                log.info("clicked", hit.entity:name(), "at", hit.point)
            end
        end
    end
    -- Where the ball is drawn: nil behind the camera, outside the size beside it.
    local screen = if self.ball then camera.world_to_screen(self.ball:world_position()) else nil
    local size = camera.screen_size()
    self.ball_visible = screen ~= nil
        and screen.x >= 0 and screen.x <= size.x
        and screen.y >= 0 and screen.y <= size.y
end

return Pick
