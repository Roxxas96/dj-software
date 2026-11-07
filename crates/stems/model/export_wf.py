# Export a weight-free HTDemucs core graph (docs/stems-backend.md, "The export").
# usage: python -I export_wf.py <955717e8.safetensors> <out_weightfree.onnx> [--perturb]
# Package versions: requirements.txt. The result must hash to the SHA-256 in the doc;
# --perturb (every weight changed) must give the same bytes, which shows no weight is inside.
# Every model parameter becomes a graph *input* named exactly as its safetensors key, dtype float16,
# shape as in safetensors, followed by Cast(to=float). onnxslim then runs with the weights as inputs,
# so it can fold shape arithmetic but never a weight.
import sys,os,subprocess,tempfile,torch,onnx
from onnx import helper,TensorProto
sys.path.insert(0,os.path.dirname(os.path.abspath(__file__)))
from htdemucs_core import Core
from demucs.hf import load_safetensors_model
torch.backends.mha.set_fastpath_enabled(False)
torch.manual_seed(0)
m=load_safetensors_model(sys.argv[1]).eval()
if '--perturb' in sys.argv:
    with torch.no_grad():
        for p in m.parameters(): p.mul_(1.01).add_(0.001)
L=int(m.segment*m.samplerate)
mix=torch.randn(1,2,L); mag=m._magnitude(m._spec(mix))
tmp=tempfile.mkdtemp()
raw=os.path.join(tmp,'raw.onnx')
with torch.no_grad():
    torch.onnx.export(Core(m).eval(),(mix,mag),raw,input_names=['mix','mag'],output_names=['spec_out','time_out'],
        opset_version=17,dynamo=False,do_constant_folding=False)
g=onnx.load(raw); G=g.graph
keys=set(m.state_dict())
kept=[];casts=[];nw=0
for init in G.initializer:
    key=init.name[2:] if init.name.startswith('m.') else None
    if key in keys:
        G.input.append(helper.make_tensor_value_info(key,TensorProto.FLOAT16,list(init.dims)))
        casts.append(helper.make_node('Cast',[key],[init.name],to=TensorProto.FLOAT,name='cast/'+key)); nw+=1
    else:
        kept.append(init)
missing=keys-{i.name[2:] for i in G.initializer if i.name.startswith('m.')}
assert not missing, ('parameters not exported as initializers', sorted(missing)[:10])
del G.initializer[:]; G.initializer.extend(kept)
nodes=list(G.node); del G.node[:]; G.node.extend(casts+nodes)
pre=os.path.join(tmp,'pre.onnx'); onnx.save(g,pre)
print('weights turned into inputs:',nw,'non-weight initializers kept before slim:',len(kept))
subprocess.run([os.path.join(os.path.dirname(sys.executable),'onnxslim'),pre,sys.argv[2]],check=True,stdout=subprocess.DEVNULL)
out=onnx.load(sys.argv[2])
print('after slim: inputs',len(out.graph.input),'initializers',len(out.graph.initializer),'nodes',len(out.graph.node))
