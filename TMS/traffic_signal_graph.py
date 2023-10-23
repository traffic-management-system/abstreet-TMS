import matplotlib.pyplot as plt
from matplotlib.animation import FuncAnimation
import pandas


fig = None 
ax = plt.subplots()

x = []
y = []

def update(frame):
    data = pandas.read_csv("data.csv")
    x = data["x"]
    y = data["y"]

    plt.cla()
    plt.plot(x, y)

    plt.legend(loc="upper left")
    plt.tight_layout()

ani = FuncAnimation(plt.gcf(), update, interval = 1000)
plt.tight_layout()
plt.show()
