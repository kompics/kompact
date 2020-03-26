# Introduction

In this section of the tutorial we will discuss the model assumptions and concepts that are underlying the implementation of Kompact. We will look at message-passing programming models, the ideas of actor references and *channels* with *ports*, and the notion of *exclusive local state*.

At a high level, Kompact is simply a merger of the Actor model of programming with the (Kompics) component model of programming. In both models light-weight processes with their own internal state communicate by exchanging discrete pieces of information (*messages* or *events*) instead of accessing shared memory structures or each other's internal state. 

While both models are formally equivalent, that is each model can be expressed in terms of the other, their different semantics can have significant impact on the performance of any implementation. Kompact thus allows programmers to express services and application in a mix of both models, thus taking advantage of their respective strengths and weaknesses as appropriate.
