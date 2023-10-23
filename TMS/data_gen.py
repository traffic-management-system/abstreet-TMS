import csv
import random
import time
import abstreet_runner

x = 0
y = 0

fieldnames = ["x", "y"]

runner = abstreet_runner.runner()

with open("data.csv", "w") as csv_file:
    csv_writer = csv.DictWriter(csv_file, fieldnames=fieldnames)
    csv_writer.writeheader()

while True:

    with open("data.csv", "a") as csv_file:
        csv_writer = csv.DictWriter(csv_file, fieldnames=fieldnames)

        info = {
            "x": x,
            "y": y
        }

        csv_writer.writerow(info)
        print(x," ", y)
        
        data = runner.get_signal_state()
        x = data[0]
        y = data[1]

        runner.increment_time()
        
    time.sleep(1)
