import requests
import datetime
from enum import Enum

class abstreet_runner:
    server_is_running = True
    timer = datetime.datetime(1, 1, 1) # filled just to avoid errors 
    
    def __init__(self):
        r = requests.get("http://localhost:5000/sim/reset")
        
        if r.status_code != 200:
            print("ERROR: abstreet server not running")
            self.server_is_running = False

    def increment_time(self):
        self.timer = self.timer + datetime.timedelta(seconds=3) # update by 3 seconds everytime
        time = self.timer.strftime("%H:%M:%S")
        r = requests.get(f"http://localhost:5000/sim/goto-time?t={time}")

        if r.status_code != 200:
            print("ERROR: could not update simulation time")
    
    def get_signal_state(self):
        t = self.timer.time()
        current_time_in_seconds = (t.hour * 60 + t.minute) * 60 + t.second
        state = None

        all_signals = requests.get("http://localhost:5000/traffic-signals/get-all-current-state").json() # this is bad and will probably cause memory issues,
        if all_signals['16916']['current_stage_idx'] == 0:                                               # but there's no other way to do it 
            state = traffic_signal.GREEN
        else:
            state = traffic_signal.RED

        coordinates[0] = current_time_in_seconds
        coordinates[1] = state

        return coordinates

class traffic_signal(Enum):
    GREEN = 1
    RED = 2
    YELLOW = 3
