-- Turning with the "look" action and locking the cursor (reference: input).
local Look = {}

function Look.on_update(self, dt)
    local look = input.axis2("look")             -- x right, y up
    self.entity:rotate_y(-look.x * 2 * dt)
    if input.mouse_pressed("Left") then
        input.lock_cursor()
    elseif input.key_pressed("Escape") then
        input.release_cursor()
    end
end

return Look
