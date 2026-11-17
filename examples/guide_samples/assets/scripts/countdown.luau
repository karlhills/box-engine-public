-- Value types, logging and a task with waits (reference: core).
local Countdown = {}

function Countdown.on_start(self)
    local axis = vector.create(0, 1, 0)
    local turn = Quat.from_axis_angle(axis, math.rad(90))
    local ahead = turn * vector.create(0, 0, -1)   -- (-1, 0, 0)
    log.info("ahead is", ahead, "tint", Color.hex("#ff8800").r)

    task.spawn(function()
        for i = 3, 1, -1 do
            log.info("starting in", i)
            wait(1)
        end
        log.info("go!")
    end)
end

return Countdown
