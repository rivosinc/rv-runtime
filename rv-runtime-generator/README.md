# Brief about data structures used.

At the root of these structures is a per-hart RISC-V architectural register tp
(thread pointer). Whenever the hart is executing the component that integrates
the rv runtime generated code (a.k.a. integrating component), this register
holds the address of a TpBlock structure which is per-hart. When the execution
is transferred out of this component (assumption is that the control is
transferred to a lower privilege a.k.a. lower privilege component), rv runtime
generated code stashes the address from tp into scratch CSR(mscratch or sscratch
depending upon the mode where the integrating component is running). This is
because lower privilege component can use tp register for its own purpose.
So, the integrating component needs a way to get to its TpBlock structure
pointer on next re-entry. Thus, the scratch CSR is utilized for the purpose of
saving tp value.

TpBlock structure contains areas for different kinds of per-hart information:
1.  Current mode stack: Value of stack pointer(sp) used by the integrating
    component.
2.  Interrupted mode stack: Value of stack pointer(sp) for the execution which
    was interrupted. This is used as a temporary storage when control comes
    to rv runtime generated code (via reset, trap entry or context switch).
    This value gets stored into the trap frame that rv runtime generated code
    prepares.
3.  Interrupted mode tp: Value of thread pointer(tp) register for the execution
    which was interrupted. This is used as a temporary storage to free up tp
    register usage until trap frame is created by rv runtime generated code.
4.  Rust EP: Entry point to rust function. Temporary storage holding the address
    of the high-level language entrypoint that the rv runtime code jumps to
    after creating trap frame. Since the trap frame creation helper can be
    called by different paths, this member allows putting in the address where
    the common code should jump to when we are ready to enter high-level
    language.
5.  Boot id: Logical ID assigned to the hart by rv runtime generated code.
6.  Hart id: Physical ID of the hart (basically mhartid value)
7.  Current context: Address of the context structure for the thread
    currently executing on this hart. This structure is expected to hold a
    ThreadContext structure at offset 0 for rv runtime usage. That is where
    rv runtime generated code dumps priv context frame address on context
    switch.
8.  Return address: Temporary storage to hold value of return address (ra)
    register. This is to make another register available when rv runtime
    generated code is entered.
9.  RT flags: Temporary storage to hold RT flags. These are stashed here
    until the rv runtime generated code creates a trap frame and moves the
    flags there.
10. Trap context address: Address of the trap context frame created for the
    current thread of execution. This provides quick access to the integrating
    component to query the reason for trap and to update any state when
    returning back from trap.

```
                                                                             Contexts                                              
          TpBlock
    +-----------------------+                       +---------+  +---------+  +---------+  +---------+  +---------+                
    |                       |                       |Priv     |  |Priv     |  |Priv     |  |Priv     |  |Priv     |                
    |   current mode sp     |                       |frame    |  |frame    |  |frame    |  |frame    |  |frame    +----------+     
    |                       |                       |addr     |  |addr     |  |addr     |  |addr     |  |addr     |          |     
    |                       |                       |---------|  |---------|  |---------|  |---------|  +---------+          |     
    +-----------------------+                       |         |  |         |  |         |  |         |  |         |          |     
    |                       |                       |         |  |         |  |         |  |         |  |         |          |     
    |  interrupted mode sp  |                       |         |  |         |  |         |  |         |  |         |          |     
    |   (temp storage)      |                       +---------+  +---------+  +---------+  +---------+  +^--------+          |     
    |                       |                                                                            |                   |     
    +-----------------------+                                                                            |             +-----v----+
    |                       |                                                                            |             |          |
    |  interrupted mode tp  |                                                                            |             | Priv     |
    |   (temp storage)      |                                                                            |             | ctx      |
    |                       |                                                                            |             | frame    |
    +-----------------------+                                                                            |             |          |
    |                       |                                                                            |             |          |
    |  rust entrypoint      |                                                                            |             +----------+
    |   (temp storage)      |                                                                            |                         
    |                       |                                                                            |                         
    +-----------------------+                                                                            |                         
    |                       |                                                                            |                         
    |     boot id           |                                                                            |                         
    |                       |                                                                            |                         
    +-----------------------+                                                                            |                         
    |                       |                                                                            |                         
    |     hart id           |                                                                            |                         
    |                       |                                                                            |                         
    +-----------------------+                                                                            |                         
    |                       |                                                                            |                         
    |   current context     +----------------------------------------------------------------------------+                         
    |                       |                                                                                                      
    +-----------------------+                                                                                                      
    |                       |                                                                                                      
    |  return address (ra)  |                                                                                                      
    |   (temp storage)      |                                                                                                      
    |                       |                                                                                                      
    +-----------------------+                                                                                                      
    |                       |                                                                                     +----------+     
    |     rt flags          |                                                                         +----------->          |     
    |   (temp storage)      |                                                                         |           | Trap     |     
    |                       |                                                                         |           | ctx      |     
    +-----------------------+                                                                         |           | frame    |     
    |                       +-------------------------------------------------------------------------+           |          |     
    | trap ctx frame address|                                                                                     |          |     
    |                       |                                                                                     |          |     
    +-----------------------+                                                                                     +----------+     

```
